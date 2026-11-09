// The catalog worker's sync (`design/general.md` §7.1): one SQLite file per library on OPFS, kept
// current from the library's change feed and the account's playlist feed. A library with no
// file is bootstrapped from its snapshot; a file that cannot be trusted is deleted and
// bootstrapped again, because the device copy is only a cache of the server. Plain TypeScript
// over `catalog-wasm`, so it runs in the worker and in a test worker alike; `worker.ts` is the
// shell around it. The caller initialises the wasm module first.

import {
	Catalog,
	Import,
	schemaVersion,
	storeNames,
	storePause,
	storeRemove,
	storeReserve
} from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';
import type { RealtimeServerMessage } from '$lib/api/types';

/** The page size asked of both feeds: the API's largest. */
const PAGE = 1000;
/** A catch-up of more than this share of a library's entities downloads its snapshot again. */
const REDOWNLOAD_SHARE = 0.1;
/** Downloads of a snapshot that did not match its ETag or would not inflate, before giving up. */
const DOWNLOAD_TRIES = 3;
/**
 * Answers of `503 catalog_building` waited out, before leaving the library to the next pull; and
 * answers of a snapshot no newer than the copy, which the server serves while it builds a newer
 * one, before catching up from the feed instead.
 */
const BUILD_TRIES = 10;
/** Seconds between asks for a snapshot newer than the copy, unless the server says otherwise. */
const REBUILD_WAIT = 5;
/**
 * Staged changes applied in one transaction, the worker answering between chunks. A chunk takes
 * about 14 ms on average and 41 ms at worst natively at 500,000 tracks, so roughly 85 ms and
 * 250 ms on a phone (wasm, 4× slower CPU). Online reads of the applying library go to the server
 * meanwhile, so a chunk holds up only other libraries' reads and offline reads; smaller chunks
 * would cost more per-transaction work and journal writes for that.
 */
const APPLY_CHUNK = 50;
/** Bytes copied from a download into the pool at a time. */
const CHUNK = 1 << 20;
/** Tries at reading or deleting the files, which fail while another worker holds the pool. */
const WIPE_TRIES = 5;
/** How long to wait between those tries. */
const WIPE_WAIT_MS = 300;
/**
 * A download's file name: a plain OPFS file beside the pool, which the pool never sees, holding
 * the gzipped snapshot of library `id` whose ETag is `etag`, or as much of it as has arrived.
 */
const DOWNLOAD = (id: string, etag: string) => `download-${id}-${etag}.tmp`;
/** Matches a `DOWNLOAD` name, giving its library and ETag. */
const DOWNLOADS = /^download-(\d+)-([0-9a-f]+)\.tmp$/;
/** An ETag the device can check a snapshot against: a SHA-256 in lowercase hex. */
const ETAG = /^[0-9a-f]{64}$/;

/** The feeds' positions a file keeps: its library feed, its account feed, and its owner. */
const LIBRARY = 'library';
const ACCOUNT = 'account';
/** A file is usable only once this is the signed-in account: its snapshot and playlists are in. */
const OWNER = 'owner';

/** A database file's name: `lib-<library>-<generation>.db`, and its journal with `-journal`. */
const FILE = /^lib-(\d+)-(\d+)\.db(-journal)?$/;

/** One entry of a feed: `rows` are the entity's rows, by table, for anything but a delete. */
type Change = { position: number; op: string; kind: string; id: string; rows?: Rows };
/** Rows by table name. */
type Rows = Record<string, Record<string, unknown>[]>;
/**
 * A page of a feed: `next` is the position of its last change, `head` that of the feed, and
 * `epoch` the server's run they belong to, which the next page is asked with.
 */
type Page = { changes: Change[]; next: number; head: number; epoch: string };

/** The messages of the realtime connection that make the worker pull. */
type Heard = Extract<
	RealtimeServerMessage,
	{ type: 'hello' | 'libraryChanged' | 'accountChanged' }
>;

/** A file's synchronous handle, which only a worker has (the DOM typings leave it out). */
type WorkerFile = FileSystemFileHandle & {
	createSyncAccessHandle(): Promise<{
		read(buffer: Uint8Array, options: { at: number }): number;
		write(buffer: Uint8Array, options: { at: number }): number;
		truncate(size: number): void;
		getSize(): number;
		flush(): void;
		close(): void;
	}>;
};

/** A download kept from an earlier try: its file, the ETag of what it holds, and its length. */
type Partial = { name: string; etag: string; size: number };

/** What went wrong that a person may need to know, such as a full disk. */
export type Report = { message: string; quota: boolean };

/** What a `CatalogSync` is made from. */
export type Options = {
	/** Where the API is, such as `/api/v1`. */
	api: string;
	/** The signed-in account's ID: a file kept for another account is deleted. */
	user: string;
	/** The `fetch` to ask the API with; tests pass a fake. */
	fetch?: typeof fetch;
	/** Told of failures worth showing; defaults to the console. */
	report?: (report: Report) => void;
	/** Whether the network may be there; pulls wait for it at the start. */
	online?: () => boolean;
	/** Told of each library whose rows on the device just changed: a catch-up, or a new snapshot. */
	changed?: (libraryId: string) => void;
};

/** A rebuild of a search index waits this many times as long as the last build took. */
const INDEX_SPACING = 10;

/** Whether the query `name` reads the search index: a search, or a recipe that starts from one. */
const usesIndex = (name: string, params: object) =>
	name === 'search' ||
	(name === 'resolveRecipe' &&
		(params as { recipe?: { type?: string } }).recipe?.type === 'search');

/** The server could not be reached, which is the normal state offline and not worth reporting. */
class Offline extends Error {}

/** The sync was closed while this was under way. */
class Closed extends Error {}

/** The server's catalog is of another schema version than this build reads: a reload fixes it. */
class Outdated extends Error {
	constructor() {
		super('the server has been updated: reload to keep its music library current on this device');
	}
}

/** The server refused a request. */
class Refused extends Error {
	constructor(readonly status: number) {
		super(`the server answered ${status}`);
	}
}

/** What to tell about a failure: its message, and whether it is the device being out of room. */
export function describe(error: unknown): Report {
	const message = error instanceof Error ? error.message : String(error);
	const quota =
		(error as { name?: string })?.name === 'QuotaExceededError' ||
		/quota|disk is full/i.test(message);
	return { message, quota };
}

/** Staged changes that would not apply: the file holds part of them, so it cannot be trusted. */
class Unapplied extends Error {}

/**
 * Whether `error` says the database file is damaged (`SQLITE_CORRUPT`, `SQLITE_NOTADB`) or holds
 * an apply that failed part-way.
 */
const damaged = (error: unknown) =>
	error instanceof Unapplied ||
	(error instanceof Error && /malformed|not a database/i.test(error.message));

/** The device's copy of the libraries, kept current from the server's feeds. */
export class CatalogSync {
	/** The libraries on this device, by ID: a library is here only once its snapshot is swapped in. */
	readonly catalogs = new Map<string, Catalog>();

	#api: string;
	#user: string;
	#fetch: typeof fetch;
	#report: (report: Report) => void;
	#online: () => boolean;
	#changed: (libraryId: string) => void;
	#abort = new AbortController();
	#closed = false;
	/** The pulls under way by name, so each feed has one at a time. */
	#runs = new Map<string, { again: boolean; done: Promise<void> }>();
	/** The libraries whose bootstrap failed, which only a start or a `hello` tries again. */
	#failed = new Set<string>();
	/** The libraries whose snapshot is of another schema version, which this worker leaves be. */
	#outdated = new Set<string>();
	/**
	 * The files part-way through applying staged changes, each with the end of its applies, which
	 * never rejects. Until then a file holds part of a catch-up.
	 */
	#applying = new Map<Catalog, Promise<void>>();
	/** The files whose apply failed part-way, which answer nothing until they are replaced. */
	#broken = new WeakSet<Catalog>();
	/** Bootstraps under way: snapshot downloads that a stop would throw away. */
	#booting = 0;
	/** The search index of each open file, which a `Catalog` holds but does not keep current. */
	#indexes = new WeakMap<Catalog, { stale: boolean; builtAt: number; took: number }>();
	/** Pool operations queued one behind another: a transfer holds the whole pool. */
	#queue: Promise<unknown> = Promise.resolve();

	/** Makes an inert sync; nothing is read or fetched until [`start`](#start). */
	constructor({
		api,
		user,
		fetch: fetcher = (input, init) => fetch(input, init),
		report = (report) => console.error(report.message),
		online = () => navigator.onLine,
		changed = () => {}
	}: Options) {
		this.#api = api;
		this.#user = user;
		this.#fetch = fetcher;
		this.#report = report;
		this.#online = online;
		this.#changed = changed;
	}

	/**
	 * Opens the files on the device, deleting any that cannot be trusted, and pulls if the network
	 * may be there. Where OPFS is unavailable the sync stays inert and the device answers
	 * nothing. Throws, also leaving it inert, if the pool cannot be read: that is a failure to
	 * report, not an absence of OPFS.
	 */
	async start() {
		if (!(await this.#hasOpfs())) {
			this.#closed = true;
			return;
		}
		try {
			const names = await this.#retrying(() => this.#names());
			const found = new Map<string, { file: string; generation: number }[]>();
			for (const name of names) {
				const match = FILE.exec(name);
				if (!match || match[3]) continue;
				found.set(match[1], [
					...(found.get(match[1]) ?? []),
					{ file: name, generation: Number(match[2]) }
				]);
			}
			await this.#reserve(2 * (found.size + 2));
			for (const [id, files] of found) {
				let kept: string | undefined;
				for (const { file } of files.sort((a, b) => b.generation - a.generation)) {
					if (kept) break;
					const catalog = await this.#adopt(file, id);
					if (catalog) this.catalogs.set(id, catalog);
					if (catalog || this.#outdated.has(id)) kept = file;
				}
				await this.#removeFiles(id, kept);
				// A worker closed mid-apply left the file part-way: it is finished before it answers.
				const catalog = this.catalogs.get(id);
				if (catalog && [LIBRARY, ACCOUNT].some((feed) => catalog.applying(feed) !== undefined)) {
					void this.#pullLibrary(id);
				}
			}
			// A library's first download goes on where a closed worker left it.
			await this.#clearDownloads((id) => !this.catalogs.has(id));
		} catch (error) {
			this.#closed = true;
			throw error;
		}
		if (this.#online()) void this.#run('all', () => this.#pullAll());
	}

	/**
	 * Hears the realtime connection: pulls on `hello`, and on a hint of a feed ahead of this
	 * device. A hint that lands during a pull makes it run once more, so none is lost.
	 */
	message(message: RealtimeServerMessage) {
		const heard = message as Heard;
		if (heard.type === 'hello') {
			void this.#run('all', () => this.#pullAll(), true);
		} else if (heard.type === 'libraryChanged') {
			// The cursor is read from the file, so not while a pool operation is under way.
			void this.#pool(async () => {
				const catalog = this.catalogs.get(heard.libraryId);
				if (!catalog) {
					if (!this.#failed.has(heard.libraryId))
						void this.#run(ACCOUNT, () => this.#pullAccount(), true);
				} else if (Number(catalog.cursor(LIBRARY)) < heard.position) {
					void this.#pullLibrary(heard.libraryId, true);
				}
			});
		} else if (heard.type === 'accountChanged') {
			void this.#run(ACCOUNT, () => this.#pullAccount(), true);
		}
	}

	/**
	 * Answers the operation `name` for `library` from its file, as `Catalog.query` does. Throws
	 * where the library is not on the device. Unless the caller is `offline`, it also throws at
	 * once where the file is part-way through an apply or no longer kept current (another schema
	 * version), so the caller asks the server; offline, it waits for the apply and answers from
	 * the copy. A file found damaged is deleted and downloaded again in the background, and the
	 * error is still thrown.
	 */
	async query(library: string, name: string, params: object, offline = false) {
		await this.#queue;
		let catalog = this.catalogs.get(library);
		if (catalog && !offline && (this.#applying.has(catalog) || this.#outdated.has(library))) {
			throw new Error('the library is being updated on this device');
		}
		while (catalog && this.#applying.has(catalog)) {
			await this.#applying.get(catalog);
			catalog = this.catalogs.get(library);
		}
		if (!catalog || this.#broken.has(catalog)) throw new Error('the library is not on this device');
		try {
			if (usesIndex(name, params)) this.#ensureIndex(library, catalog);
			return catalog.query(this.#user, library, name, JSON.stringify(params));
		} catch (error) {
			if (damaged(error)) void this.#run(`lib:${library}`, () => this.#rebuild(library));
			throw error;
		}
	}

	/** Tells the page the library changed, and marks its search index out of date. */
	#announce(library: string) {
		const catalog = this.catalogs.get(library);
		const index = catalog && this.#indexes.get(catalog);
		if (index) index.stale = true;
		this.#changed(library);
	}

	/**
	 * Makes sure `catalog` has a search index: the first search builds it and waits. A later one
	 * on a changed library answers from the index it has and schedules a rebuild once the file
	 * has gone `INDEX_SPACING` times as long as the last build took, as the server spaces its own
	 * (`api/search/indexes.rs`), so a scan costs a tenth of the worker at most.
	 */
	#ensureIndex(library: string, catalog: Catalog) {
		const index = this.#indexes.get(catalog);
		if (!index) {
			this.#build(library, catalog);
		} else if (index.stale && performance.now() - index.builtAt >= index.took * INDEX_SPACING) {
			index.stale = false;
			setTimeout(async () => {
				await this.#queue;
				if (this.#closed || this.catalogs.get(library) !== catalog) return;
				// A file part-way through an apply is indexed once the apply announces itself.
				if (this.#applying.has(catalog)) {
					index.stale = true;
					return;
				}
				try {
					this.#build(library, catalog);
				} catch (error) {
					index.stale = true;
					this.#report(describe(error));
				}
			});
		}
	}

	/** Builds `catalog`'s search index now, recording how long it took. */
	#build(library: string, catalog: Catalog) {
		const started = performance.now();
		catalog.buildIndex(library);
		const builtAt = performance.now();
		this.#indexes.set(catalog, { stale: false, builtAt, took: builtAt - started });
	}

	/**
	 * Whether `library` has a file on the device that can answer: one whose apply failed part-way
	 * does not. Whether it answers now depends on the caller being offline; see [`query`](#query).
	 */
	onDevice(library: string) {
		const catalog = this.catalogs.get(library);
		return catalog !== undefined && !this.#broken.has(catalog);
	}

	/** Resolves once no pull is under way. */
	async idle() {
		while (this.#runs.size) await Promise.all([...this.#runs.values()].map((run) => run.done));
	}

	/** Stops pulling and lets go of the files, which stay on the device. */
	async close() {
		this.#closed = true;
		this.#abort.abort();
		await this.idle();
		for (const catalog of this.catalogs.values()) catalog.free();
		this.catalogs.clear();
	}

	/**
	 * Closes and releases the pool to another tab's worker, which this one must not outlive. The
	 * files stay.
	 */
	async pause() {
		await this.close();
		storePause();
	}

	/**
	 * Closes, then deletes every file: nothing of the account stays on the device. A pool another
	 * worker still holds is tried again a few times; if the files stay, it throws.
	 */
	async signOut() {
		await this.close();
		if (!(await this.#hasOpfs())) return;
		await this.#retrying(async () => {
			for (const name of await this.#names()) await this.#remove(name);
			await this.#clearDownloads(() => false);
		});
	}

	/** Runs `job`, trying again a few times while another worker still holds the pool. */
	async #retrying<T>(job: () => Promise<T>): Promise<T> {
		for (let attempt = 1; ; attempt++) {
			try {
				return await job();
			} catch (error) {
				if (attempt === WIPE_TRIES) throw error;
				await new Promise((resolve) => setTimeout(resolve, WIPE_WAIT_MS));
			}
		}
	}

	/** Reads the library list, then every feed: what the start and a `hello` ask for. */
	async #pullAll() {
		this.#failed.clear();
		const held = [...this.catalogs.keys()];
		await this.#run(ACCOUNT, () => this.#pullAccount(), true);
		await Promise.all(
			held.filter((id) => this.catalogs.has(id)).map((id) => this.#pullLibrary(id, true))
		);
	}

	/**
	 * Runs `job` under `name` unless one is under way, which a request with `again` makes run
	 * once more. Failures are reported, so the returned promise never rejects.
	 */
	#run(name: string, job: () => Promise<void>, again = false) {
		const running = this.#runs.get(name);
		if (running) {
			running.again ||= again;
			return running.done;
		}
		const run = { again: false, done: Promise.resolve() };
		run.done = (async () => {
			try {
				do {
					run.again = false;
					await job();
				} while (run.again && !this.#closed);
			} catch (error) {
				this.#fail(error);
			} finally {
				this.#runs.delete(name);
			}
		})();
		this.#runs.set(name, run);
		return run.done;
	}

	/** Reports a failure other than the network's or a close's. */
	#fail(error: unknown) {
		if (error instanceof Offline || error instanceof Closed) return;
		this.#report(describe(error));
	}

	/** Throws unless the session is still this worker's account, whose files these are. */
	async #mine() {
		const me = await this.#json<{ id: string }>('/me');
		if (String(me.id) !== this.#user) throw new Error('the session is another account’s');
	}

	/**
	 * The account's pull: the library list, then each file's playlists. A library the account
	 * gained is bootstrapped in the background, so the others' catch-ups do not wait for it.
	 * Nothing is read while the session is another account's, as in a tab thawed after a
	 * sign-in elsewhere.
	 */
	async #pullAccount() {
		await this.#mine();
		const listed = new Set(
			(await this.#json<{ items: { id: string }[] }>('/libraries')).items.map(({ id }) => id)
		);
		for (const id of [...this.catalogs.keys()]) if (!listed.has(id)) await this.#discard(id);
		// One still being written cannot be deleted; its next request is refused and deletes it.
		await this.#clearDownloads((id) => listed.has(id)).catch(() => {});
		for (const id of listed) {
			if (!this.catalogs.has(id) && !this.#failed.has(id)) void this.#pullLibrary(id);
		}
		for (const [id, catalog] of [...this.catalogs]) {
			if (!this.#outdated.has(id)) await this.#syncAccount(id, catalog);
		}
	}

	/** Brings `catalog`'s playlists up to the account feed's head. */
	async #syncAccount(id: string, catalog: Catalog) {
		let expired: boolean;
		try {
			expired = !(await this.#catchUpAccount(id, catalog, () => this.#gone(id, catalog)));
		} catch (error) {
			if (!damaged(error)) throw error;
			await this.#discard(id);
			void this.#pullLibrary(id);
			return;
		}
		// A repeat of the run once the new file is in is a catch-up, not another download.
		if (expired)
			void this.#run(`lib:${id}`, () => this.#syncLibrary(id, this.catalogs.get(id) === catalog));
	}

	/**
	 * Stages the account feed's pages after the file's position, only for the library's own
	 * playlists, and commits at the head, announcing the library as changed if any playlist of
	 * its own came. False if the server no longer reaches back that far.
	 */
	async #catchUpAccount(id: string, catalog: Catalog, stale: () => boolean) {
		let after = Number(catalog.pulled(ACCOUNT));
		let touched = false;
		for (;;) {
			const response = await this.#get(`/me/changes?${this.#cursor(catalog, ACCOUNT, after)}`);
			if (stale()) return true;
			if (response.status === 410) return false;
			const page = await this.#page(response);
			if (stale()) return true;
			const mine = page.changes.filter(
				(change) =>
					change.op === 'delete' ||
					Object.values(change.rows ?? {}).every((rows) =>
						rows.every((row) => String(row.library_id) === id)
					)
			);
			await this.#queue;
			catalog.stage(ACCOUNT, page.epoch, JSON.stringify(mine));
			touched ||= mine.length > 0;
			if (page.next >= page.head) {
				await this.#apply(catalog, ACCOUNT, BigInt(page.next), stale);
				// A file not yet swapped in announces itself when it is.
				if (touched && this.catalogs.get(id) === catalog) this.#changed(id);
				return true;
			}
			after = page.next;
		}
	}

	/** Starts the pull of a library's feed, which `again` repeats if one is under way. */
	#pullLibrary(id: string, again = false) {
		return this.#run(`lib:${id}`, () => this.#syncLibrary(id), again);
	}

	/**
	 * Brings a library to its feed's head: bootstrapping it first if it has no file, and
	 * downloading its snapshot again when the feed cannot be followed or is too long. `refresh`
	 * starts with a snapshot, as when the account feed no longer reaches back.
	 */
	async #syncLibrary(id: string, refresh = false) {
		if (this.#outdated.has(id)) return;
		let rebuilds = 0;
		let fresh = false;
		let outcome: 'current' | 'far' | 'expired' | 'damaged' = refresh ? 'expired' : 'current';
		const held = this.catalogs.get(id);
		try {
			if (held) await this.#finish(id, held);
		} catch (error) {
			if (!damaged(error)) throw error;
			outcome = 'damaged';
		}
		for (;;) {
			let catalog = this.catalogs.get(id);
			if (!catalog || outcome !== 'current') {
				if (catalog && outcome === 'damaged') {
					await this.#discard(id);
				}
				// A snapshot behind the feed's own reach replaces the copy; the copy serves until then.
				// A feed that keeps damaging fresh copies leaves the library to the server.
				if ((outcome === 'expired' || outcome === 'damaged') && ++rebuilds > 2) {
					throw new Error('the library cannot be brought up to date');
				}
				catalog = await this.#boot(id, outcome === 'expired');
				if (!catalog) return;
				fresh = true;
			}
			try {
				outcome = await this.#catchUp(id, catalog, fresh);
			} catch (error) {
				if (!damaged(error)) throw error;
				outcome = 'damaged';
			}
			if (this.#gone(id, catalog)) return;
			if (outcome === 'current') return;
		}
	}

	/** Bootstraps `id`, remembering a failure so only a start or a `hello` tries again. */
	async #boot(id: string, forced: boolean) {
		try {
			const catalog = await this.#bootstrap(id, forced);
			this.#failed.delete(id);
			return catalog;
		} catch (error) {
			this.#failed.add(id);
			throw error;
		}
	}

	/**
	 * Stages a library's feed pages after the file's position and commits at the head. `far` as
	 * soon as the changes pulled are more than a tenth of the library, which a snapshot is
	 * quicker than, unless `fresh` says the file is only just downloaded; `expired` when the
	 * server no longer reaches back that far. Announces the library as changed when it commits any.
	 */
	async #catchUp(id: string, catalog: Catalog, fresh: boolean) {
		const stale = () => this.#gone(id, catalog);
		let after = Number(catalog.pulled(LIBRARY));
		let pulled = 0;
		for (;;) {
			const response = await this.#get(
				`/libraries/${id}/changes?${this.#cursor(catalog, LIBRARY, after)}`
			);
			if (stale()) return 'current';
			if (response.status === 410) return 'expired';
			if (response.status === 404) {
				await this.#discard(id);
				return 'current';
			}
			const page = await this.#page(response);
			if (stale()) return 'current';
			pulled += page.changes.length;
			await this.#queue;
			if (!fresh && pulled > this.#entities(id, catalog) * REDOWNLOAD_SHARE) return 'far';
			catalog.stage(LIBRARY, page.epoch, JSON.stringify(page.changes));
			if (page.next >= page.head) {
				await this.#apply(catalog, LIBRARY, BigInt(page.next), stale);
				if (pulled > 0 && !stale()) this.#announce(id);
				return 'current';
			}
			after = page.next;
		}
	}

	/**
	 * Applies what `catalog` has staged for `feed` and moves its position to `through`, a chunk a
	 * transaction, yielding between chunks so the worker answers meanwhile. Until it ends, reads
	 * of the file go to the server or wait ([`onDevice`](#onDevice)). Stops once `stale` says the
	 * file is not the one to write, leaving the file's mark for the next pull to finish. A chunk
	 * that fails marks the file broken, so it answers nothing until an apply completes, and throws
	 * [`Unapplied`], which callers treat as damage; or, where the device is full, the error itself,
	 * reported as a quota error with the copy kept.
	 */
	async #apply(catalog: Catalog, feed: string, through: bigint, stale: () => boolean) {
		const before = this.#applying.get(catalog);
		const run = (async () => {
			await before;
			while (!stale()) {
				let done: boolean;
				try {
					done = catalog.commit(feed, through, APPLY_CHUNK);
				} catch (error) {
					this.#broken.add(catalog);
					// A full device leaves the file whole, if part-way: a later pull finishes it.
					if (describe(error).quota) throw error;
					throw new Unapplied(`staged changes would not apply: ${describe(error).message}`);
				}
				if (done) {
					this.#broken.delete(catalog);
					return;
				}
				await new Promise((resolve) => setTimeout(resolve));
				await this.#queue;
			}
		})();
		const end = run.catch(() => {});
		this.#applying.set(catalog, end);
		try {
			await run;
		} finally {
			if (this.#applying.get(catalog) === end) this.#applying.delete(catalog);
		}
	}

	/** Finishes the applies a closed worker left part-way in the library's file `catalog`. */
	async #finish(id: string, catalog: Catalog) {
		const stale = () => this.#gone(id, catalog);
		for (const feed of [LIBRARY, ACCOUNT]) {
			const through = stale() ? undefined : catalog.applying(feed);
			if (through !== undefined) await this.#apply(catalog, feed, through, stale);
		}
	}

	/**
	 * The query of a page of `feed` after position `after`: with the epoch `catalog` holds for it,
	 * since the server goes on only from a position of a run it knows.
	 */
	#cursor(catalog: Catalog, feed: string, after: number) {
		const epoch = after > 0 ? catalog.epoch(feed) : undefined;
		return `after=${after}${epoch ? `&epoch=${epoch}` : ''}&limit=${PAGE}`;
	}

	/** The number of tracks, albums and artists in a library, from the totals of its listings. */
	#entities(id: string, catalog: Catalog) {
		return ['listTracks', 'listAlbums', 'listArtists']
			.map((name) => JSON.parse(catalog.query(this.#user, id, name, '{"limit":1}')).total as number)
			.reduce((sum, total) => sum + total, 0);
	}

	/** Whether `catalog` is no longer the library's file, or the sync is closed. */
	#gone(id: string, catalog: Catalog) {
		return this.#closed || this.catalogs.get(id) !== catalog;
	}

	/** Deletes a library's file and downloads it again. */
	async #rebuild(id: string) {
		await this.#discard(id);
		await this.#syncLibrary(id);
	}

	/** Lets go of a library and deletes its files. */
	async #discard(id: string) {
		this.catalogs.get(id)?.free();
		this.catalogs.delete(id);
		await this.#removeFiles(id);
	}

	/** Deletes the files of library `id`, journals included, except `keep` and its journal. */
	async #removeFiles(id: string, keep?: string) {
		for (const name of await this.#names()) {
			const match = FILE.exec(name);
			if (match?.[1] !== id || name.replace(/-journal$/, '') === keep) continue;
			await this.#remove(name);
		}
	}

	/**
	 * Opens library `id`'s file found on the device, or deletes it if it cannot be trusted. A file
	 * of a newer schema version is a newer build's, open in another tab: it is kept, and the
	 * library is outdated here, so this worker neither downloads nor deletes it.
	 */
	async #adopt(file: string, id: string) {
		let catalog: Catalog | undefined;
		try {
			catalog = await this.#open(file);
			if (catalog.cursor(OWNER) === BigInt(this.#user)) return catalog;
		} catch (error) {
			if (error instanceof Error && error.message.startsWith('newer')) {
				this.#outdated.add(id);
				return undefined;
			}
			// A file that will not open, or is of an older schema version, is rebuilt.
		}
		catalog?.free();
		await this.#remove(file);
		await this.#remove(`${file}-journal`);
		return undefined;
	}

	/** Whether a snapshot is being downloaded or installed, which stopping now would waste. */
	busy() {
		return this.#booting > 0;
	}

	/** Bootstraps `id`, counting it as under way; see [`#install`](#install). */
	async #bootstrap(id: string, forced: boolean) {
		this.#booting++;
		try {
			return await this.#install(id, forced);
		} finally {
			this.#booting--;
		}
	}

	/**
	 * Downloads a library's snapshot, imports it as a new file, adds the library's playlists to
	 * it, and swaps it in for any copy the library has, announcing the library as changed.
	 * Nothing shows until the swap. Undefined when the account does not reach the library. Unless `forced`, a snapshot no newer than the
	 * copy's own position is not imported: it is asked for again while the server builds a newer
	 * one, and the copy is returned if none comes.
	 */
	async #install(id: string, forced: boolean) {
		await this.#mine();
		const existing = this.catalogs.get(id);
		const generation = (await this.#names()).reduce((latest, name) => {
			const match = FILE.exec(name);
			return match?.[1] === id ? Math.max(latest, Number(match[2])) : latest;
		}, 0);
		const file = `lib-${id}-${generation + 1}.db`;
		await this.#reserve(2 * (this.catalogs.size + 3));
		let position: number;
		let epoch: string;
		for (let mismatches = 0, older = 0; ;) {
			const partial = await this.#partial(id);
			const response = await this.#snapshot(id, partial);
			if (!response) {
				await this.#discard(id);
				return undefined;
			}
			position = Number(response.headers.get('x-jewelcase-position'));
			epoch = response.headers.get('x-jewelcase-epoch') ?? '';
			if (existing && !forced && position <= Number(existing.cursor(LIBRARY))) {
				if (this.catalogs.get(id) === existing) {
					void response.body?.cancel();
					if (++older === BUILD_TRIES) return existing;
					// Asking for a stale snapshot starts the build of a newer one.
					await this.#sleep(this.#retryAfter(response, REBUILD_WAIT));
					continue;
				}
			}
			try {
				await this.#import(id, file, response, partial);
				break;
			} catch (error) {
				const mismatch =
					error instanceof Error && /snapshot (hash mismatch|damaged)/.test(error.message);
				if (!mismatch || ++mismatches === DOWNLOAD_TRIES) throw error;
			}
		}
		let catalog: Catalog | undefined;
		try {
			catalog = await this.#open(file);
			if (!(await this.#catchUpAccount(id, catalog, () => this.#closed))) {
				throw new Error('the account feed refused a start from the beginning');
			}
			await this.#queue;
			// A new file stages nothing for these, so each is done in one call.
			catalog.stage(LIBRARY, epoch, '[]');
			catalog.commit(LIBRARY, BigInt(position), APPLY_CHUNK);
			catalog.commit(OWNER, BigInt(this.#user), APPLY_CHUNK);
		} catch (error) {
			catalog?.free();
			await this.#remove(file);
			throw error;
		}
		if (this.#closed) {
			catalog.free();
			await this.#remove(file);
			throw new Closed();
		}
		this.catalogs.get(id)?.free();
		this.catalogs.set(id, catalog);
		await this.#removeFiles(id, file);
		this.#announce(id);
		return catalog;
	}

	/**
	 * The snapshot's response, waiting out `503 catalog_building`; null where the account does
	 * not reach the library, which deletes any `partial` download. With `partial`, only the rest
	 * of the file it is part of is asked for, and a server whose file has changed since sends the
	 * new one whole. A snapshot of another schema version than this build's stops the library's
	 * sync for good: only a reload, which brings the new build, helps.
	 */
	async #snapshot(id: string, partial?: Partial) {
		for (let tries = 0; tries < BUILD_TRIES; tries++) {
			const headers: Record<string, string> = partial
				? { range: `bytes=${partial.size}-`, 'if-range': `"${partial.etag}"` }
				: {};
			const response = await this.#get(`/libraries/${id}/catalog`, headers);
			if (response.status === 404 || response.status === 416) await this.#unlink(partial?.name);
			if (response.status === 404) return null;
			// The download is whole and only its import was cut short: it is asked for again whole.
			if (response.status === 416 && partial) {
				partial = undefined;
				continue;
			}
			if (response.status !== 503) {
				if (!response.ok) throw new Refused(response.status);
				if (Number(response.headers.get('x-jewelcase-schema')) !== schemaVersion()) {
					void response.body?.cancel();
					this.#outdated.add(id);
					throw new Outdated();
				}
				return response;
			}
			await this.#sleep(this.#retryAfter(response, 1));
		}
		// Still building: a later pull asks again.
		throw new Offline();
	}

	/**
	 * Imports the gzipped snapshot of `response` into the pool as `file`. The body is first
	 * written to a plain OPFS file beside the pool, so the network never holds the pool, which a
	 * transfer takes whole; then one quick local pass inflates it in, hashing the gzip as it goes,
	 * and publishes it only if the hash is the ETag. A `206` goes on from the end of `partial`,
	 * the download an earlier try left; any other answer replaces it. A body that breaks off
	 * leaves its download for the next try to go on from; anything else deletes it.
	 */
	async #import(id: string, file: string, response: Response, partial?: Partial) {
		const etag = response.headers.get('etag')?.replaceAll('"', '');
		if (!etag || !ETAG.test(etag) || !response.body) {
			throw new Error('the catalog file has no ETag');
		}
		const start = Number(/^bytes (\d+)-/.exec(response.headers.get('content-range') ?? '')?.[1]);
		const resumed = response.status === 206 && partial?.etag === etag && start === partial.size;
		if (!resumed) await this.#unlink(partial?.name);
		if (response.status === 206 && !resumed) {
			void response.body.cancel();
			throw new Error('the server sent part of a catalog file it was not asked for');
		}
		const root = await navigator.storage.getDirectory();
		const name = DOWNLOAD(id, etag);
		let cut = false;
		try {
			const handle = (await root.getFileHandle(name, { create: true })) as WorkerFile;
			const download = await handle.createSyncAccessHandle();
			try {
				let at = resumed ? start : 0;
				download.truncate(at);
				const reader = response.body.getReader();
				for (;;) {
					const { done, value } = await reader.read().catch(() => {
						cut = true;
						throw this.#closed ? new Closed() : new Offline();
					});
					if (done) break;
					if (download.write(value, { at }) !== value.length) throw new Error('the device is full');
					at += value.length;
				}
				download.flush();
				await this.#pool(async () => {
					const size = download.getSize();
					// The gzip trailer's last four bytes are the inflated size, little-endian.
					const trailer = new Uint8Array(4);
					download.read(trailer, { at: Math.max(0, size - 4) });
					const target = await Import.begin(file, new DataView(trailer.buffer).getUint32(0, true));
					try {
						const chunk = new Uint8Array(CHUNK);
						for (let from = 0; from < size;) {
							const read = download.read(chunk, { at: from });
							target.write(chunk.subarray(0, read));
							from += read;
						}
					} catch (error) {
						target.free();
						throw error;
					}
					target.finish(etag);
				});
			} finally {
				download.close();
			}
		} finally {
			if (!cut) await this.#unlink(name);
		}
	}

	/** The download an earlier try left of library `id`'s snapshot, if there is one. */
	async #partial(id: string): Promise<Partial | undefined> {
		const root = await navigator.storage.getDirectory();
		for await (const [name, handle] of root.entries()) {
			const [, library, etag] = DOWNLOADS.exec(name) ?? [];
			if (library !== id || handle.kind !== 'file') continue;
			const { size } = await (handle as FileSystemFileHandle).getFile();
			if (size > 0) return { name, etag, size };
		}
		return undefined;
	}

	/** Deletes the plain OPFS file `name`, if there is one. */
	async #unlink(name: string | undefined) {
		if (!name) return;
		const root = await navigator.storage.getDirectory();
		await root.removeEntry(name).catch(() => {});
	}

	/** Whether the browser has OPFS at all, which it does not in Safari's private browsing. */
	async #hasOpfs() {
		try {
			await navigator.storage.getDirectory();
			return true;
		} catch {
			return false;
		}
	}

	/** Deletes the downloads, except those of the libraries `keep` says yes to. */
	async #clearDownloads(keep: (id: string) => boolean) {
		const root = await navigator.storage.getDirectory();
		for await (const name of root.keys()) {
			const match = DOWNLOADS.exec(name);
			if (match && !keep(match[1])) await root.removeEntry(name);
		}
	}

	/** Runs `job` once the pool operations queued before it are done. */
	#pool<T>(job: () => Promise<T>): Promise<T> {
		const run = this.#queue.then(job, job);
		this.#queue = run.catch(() => {});
		return run;
	}

	/** The names of the files in the pool, queued behind other pool operations. */
	#names() {
		return this.#pool(() => storeNames());
	}

	/** Deletes the pool file `name`, queued behind other pool operations. */
	#remove(name: string) {
		return this.#pool(() => storeRemove(name));
	}

	/** Makes room in the pool for `files` files, queued behind other pool operations. */
	#reserve(files: number) {
		return this.#pool(() => storeReserve(files));
	}

	/** Opens the pool file `file` as a catalog, queued behind other pool operations. */
	#open(file: string) {
		return this.#pool(() => Catalog.open(file));
	}

	/** Fetches `path` of the API, with any `headers`. */
	async #get(path: string, headers?: Record<string, string>) {
		if (this.#closed) throw new Closed();
		try {
			return await this.#fetch(`${this.#api}${path}`, { signal: this.#abort.signal, headers });
		} catch {
			throw this.#closed ? new Closed() : new Offline();
		}
	}

	/** The body of a successful `response` as JSON. */
	async #page(response: Response): Promise<Page> {
		if (!response.ok) throw new Refused(response.status);
		return response.json();
	}

	/** Fetches `path` as JSON. */
	async #json<T>(path: string): Promise<T> {
		return (await this.#page(await this.#get(path))) as T;
	}

	/** The milliseconds `response`'s `Retry-After` asks to wait, or `seconds` when it has none. */
	#retryAfter(response: Response, seconds: number) {
		const after = response.headers.get('retry-after');
		return (after === null ? seconds : Number(after) || 0) * 1000;
	}

	/** Waits `ms`, or less if closed. */
	#sleep(ms: number) {
		return new Promise<void>((resolve) => {
			const timer = setTimeout(resolve, ms);
			this.#abort.signal.addEventListener(
				'abort',
				() => {
					clearTimeout(timer);
					resolve();
				},
				{ once: true }
			);
		});
	}
}
