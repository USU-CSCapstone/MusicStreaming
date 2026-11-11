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
import type { Call } from '$lib/sync/mutation';
import { ALL, collect, told, type Kind } from './kinds';
import type { LibraryState } from './protocol';

/** The page size asked of both feeds: the API's largest. */
const PAGE = 1000;
/** The kinds of rows the device's own writes change: what taking them back may change. */
const REWOUND: readonly Kind[] = ['playlists', 'liked', 'hidden'];
/** A catch-up of more than this share of a library's entities downloads its snapshot again. */
const REDOWNLOAD_SHARE = 0.1;
/** Downloads of a snapshot whose hash did not match its ETag, before giving up. */
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
 * How long one slice of an apply should hold the worker, in milliseconds. A read of another
 * library waits for at most the slice under way, and a slice that overshoots is followed by a
 * smaller one.
 */
const SLICE_MS = 50;
/**
 * Staged changes applied in one transaction: the first slice's size and the fewest in any, since
 * a commit costs a flush however few it holds. Each next slice adapts to the time the last took
 * (`slice`), up to `APPLY_MAX`. At 500,000 tracks a change takes about 0.3 ms in the browser, so
 * a slice is about 170 changes, and 50 with the CPU throttled, which holds the worker for 100 ms
 * or more.
 * Online reads of the applying library go to the server meanwhile; the worker answers everything
 * else between slices.
 */
const APPLY_CHUNK = 50;
/** The most staged changes in one slice of an apply. */
const APPLY_MAX = 1000;
/** Bytes copied from a download into the pool at a time. */
const CHUNK = 1 << 20;
/** Tries at reading or deleting the files, which fail while another worker holds the pool. */
const WIPE_TRIES = 5;
/** How long to wait between those tries. */
const WIPE_WAIT_MS = 300;
/** A download's file name: a plain OPFS file beside the pool, which the pool never sees. */
const DOWNLOAD = (id: string) => `download-${id}.tmp`;
/** Matches every name `DOWNLOAD` makes. */
const DOWNLOADS = /^download-.*\.tmp$/;

/** The feeds' positions a file keeps: its library feed, its account feed, and its owner. */
const LIBRARY = 'library';
const ACCOUNT = 'account';
/** A file is usable only once this is the signed-in account: its snapshot and playlists are in. */
const OWNER = 'owner';

/** A database file's name: `lib-<library>-<generation>.db`, and its journal with `-journal`. */
const FILE = /^lib-(\d+)-(\d+)\.db(-journal)?$/;

/** One entry of a feed: `rows` are the entity's rows, by table, for anything but a delete. */
type Change = { seq: number; op: string; kind: string; id: string; rows?: Rows };
/** Rows by table name. */
type Rows = Record<string, Record<string, unknown>[]>;
/** A page of a feed: `next` is the position of its last change, `head` that of the feed. */
type Page = { changes: Change[]; next: number; head: number };

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
		getSize(): number;
		flush(): void;
		close(): void;
	}>;
};

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
	/**
	 * Told of each library whose rows on the device just changed, and of what kinds: a catch-up, or a
	 * new snapshot (all kinds).
	 */
	changed?: (libraryId: string, kinds: readonly Kind[]) => void;
	/**
	 * Told when the worker enters and leaves work that keeps it busy for seconds: a snapshot
	 * install's copy.
	 */
	heavy?: (heavy: boolean) => void;
	/** Told what each library on the device stands at, whenever that changes. */
	state?: (libraries: Record<string, LibraryState>) => void;
	/**
	 * Told of each library the account no longer reaches, once its file is deleted, to drop what else
	 * is kept for it.
	 */
	lost?: (libraryId: string) => Promise<void> | void;
	/**
	 * The writes of the mutations the log holds for a library, in the order they were made: what the
	 * device shows over the server's rows until each is answered (`$lib/sync`).
	 */
	pending?: (libraryId: string) => Promise<Call[]>;
};

/** A rebuild of a search index waits this many times as long as the last build took. */
const INDEX_SPACING = 10;

/** Whether the query `name` reads the search index: a search, or a recipe that starts from one. */
const usesIndex = (name: string, params: object) =>
	name === 'search' ||
	(name === 'resolveRecipe' &&
		(params as { recipe?: { type?: string } }).recipe?.type === 'search');

/**
 * The size of the slice after one of `limit` that took `took` ms: the size that would take
 * `SLICE_MS`, at most twice `limit` and at most `max`, and at least 1.
 */
const slice = (limit: number, took: number, max: number) =>
	Math.max(1, Math.min(max, limit * 2, took > 0 ? Math.floor((limit * SLICE_MS) / took) : max));

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
	/**
	 * The libraries on this device, by ID: a library is here only once its snapshot is swapped in.
	 */
	readonly catalogs = new Map<string, Catalog>();

	#api: string;
	#user: string;
	#fetch: typeof fetch;
	#report: (report: Report) => void;
	#online: () => boolean;
	#changed: (libraryId: string, kinds: readonly Kind[]) => void;
	#lost: (libraryId: string) => Promise<void> | void;
	#pending: (libraryId: string) => Promise<Call[]>;
	#state: (libraries: Record<string, LibraryState>) => void;
	#heavy: (heavy: boolean) => void;
	/** The long calls under way, which `#heavy` hears of as one. */
	#holds = 0;
	/** The JSON of what `#state` was last told, so it hears only changes. */
	#told = '';
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
		changed = () => {},
		state = () => {},
		heavy = () => {},
		lost = () => {},
		pending = async () => []
	}: Options) {
		this.#api = api;
		this.#user = user;
		this.#fetch = fetcher;
		this.#report = report;
		this.#online = online;
		this.#changed = changed;
		this.#state = state;
		this.#heavy = heavy;
		this.#lost = lost;
		this.#pending = pending;
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
			await this.#clearDownloads();
			const found = new Map<string, { file: string; generation: number }[]>();
			for (const name of names) {
				const match = FILE.exec(name);
				if (!match) continue;
				// A journal alone, left by a crash, is found so that its library's files are cleaned.
				found.set(match[1], [
					...(found.get(match[1]) ?? []),
					...(match[3] ? [] : [{ file: name, generation: Number(match[2]) }])
				]);
			}
			await this.#reserve(2 * (found.size + 2));
			for (const [id, files] of found) {
				let kept: string | undefined;
				for (const { file } of files.sort((a, b) => b.generation - a.generation)) {
					if (kept) break;
					const catalog = await this.#adopt(file, id);
					if (catalog) {
						this.catalogs.set(id, catalog);
						this.#publish();
					}
					if (catalog || this.#outdated.has(id)) kept = file;
				}
				await this.#removeFiles(id, kept);
				// A worker closed mid-apply left the file part-way: it is finished before it answers.
				const catalog = this.catalogs.get(id);
				if (catalog && [LIBRARY, ACCOUNT].some((feed) => catalog.applying(feed) !== undefined)) {
					void this.#pullLibrary(id);
				}
			}
			// What a closed worker left made or unmade of the log, or a tab made with no worker to
			// make it, is made as the log has it now.
			for (const id of this.catalogs.keys()) void this.rebase(id);
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
				} else if (Number(catalog.cursor(LIBRARY)) < heard.seq) {
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

	/** Tells the page the library changed in `kinds`, and marks its search index out of date. */
	#announce(library: string, kinds: readonly Kind[]) {
		const catalog = this.catalogs.get(library);
		const index = catalog && this.#indexes.get(catalog);
		if (index) index.stale = true;
		this.#changed(library, kinds);
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
	 * What each library on the device stands at, as the page is told whenever it changes: whether
	 * it answers, and is part-way through an apply.
	 */
	state() {
		const libraries: Record<string, LibraryState> = {};
		for (const [id, catalog] of this.catalogs) {
			libraries[id] = { onDevice: this.onDevice(id), applying: this.#applying.has(catalog) };
		}
		return libraries;
	}

	/** Tells `state` what the libraries stand at now, if that is not what it was last told. */
	#publish() {
		const libraries = this.state();
		const text = JSON.stringify(libraries);
		if (text === this.#told) return;
		this.#told = text;
		this.#state(libraries);
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
		this.#publish();
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
			await this.#clearDownloads();
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

	/**
	 * Tells `heavy` the worker is entering (`true`) or leaving (`false`) work that keeps it busy
	 * for seconds: once for all of it that is under way at once.
	 */
	#hold(entering: boolean) {
		this.#holds += entering ? 1 : -1;
		if (this.#holds === (entering ? 1 : 0)) this.#heavy(entering);
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
		for (const id of [...this.catalogs.keys()]) if (!listed.has(id)) await this.#lose(id);
		for (const id of listed) {
			if (!this.catalogs.has(id) && !this.#failed.has(id)) void this.#pullLibrary(id);
		}
		for (const [id, catalog] of [...this.catalogs]) {
			// One swapped for a new copy meanwhile is caught up as that copy is installed.
			if (!this.#outdated.has(id) && !this.#gone(id, catalog)) await this.#syncAccount(id, catalog);
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
			// After any pull under way, which finds the file gone and leaves it so.
			void this.#pullLibrary(id, true);
			return;
		}
		if (expired) void this.#run(`lib:${id}`, () => this.#syncLibrary(id, true));
	}

	/**
	 * Stages the account feed's pages after the file's position, only for the library's own
	 * playlists, and commits at the head, announcing the library as changed if any playlist of
	 * its own came. False if the server no longer reaches back that far.
	 */
	async #catchUpAccount(id: string, catalog: Catalog, stale: () => boolean) {
		if (stale()) return true;
		let after = Number(catalog.pulled(ACCOUNT));
		const kinds = new Set<Kind>();
		const liked = catalog.liked(this.#user, id);
		let staged = 0;
		for (;;) {
			const response = await this.#get(`/me/changes?after=${after}&limit=${PAGE}`);
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
			if (stale()) return true;
			catalog.stage(ACCOUNT, JSON.stringify(mine));
			collect(kinds, mine, liked);
			staged += mine.length;
			if (page.next >= page.head) {
				// A new file not swapped in yet has the device's own writes made as it is installed.
				const rebase = staged > 0 && this.catalogs.get(id) === catalog;
				await this.#apply(id, catalog, ACCOUNT, BigInt(page.next), stale, rebase);
				// A file not yet swapped in announces itself when it is.
				if (kinds.size > 0 && this.catalogs.get(id) === catalog) this.#changed(id, told(kinds));
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
		// A session that is another account's pulls nothing of it, and deletes nothing on its 404s. The
		// file is read next, never while a pool operation is under way, as one may be after the wait.
		await this.#mine();
		await this.#queue;
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
		const kinds = new Set<Kind>();
		for (;;) {
			const response = await this.#get(`/libraries/${id}/changes?after=${after}&limit=${PAGE}`);
			if (stale()) return 'current';
			if (response.status === 410) return 'expired';
			if (response.status === 404) {
				await this.#lose(id);
				return 'current';
			}
			const page = await this.#page(response);
			if (stale()) return 'current';
			pulled += page.changes.length;
			collect(kinds, page.changes);
			await this.#queue;
			if (!fresh && pulled > this.#entities(id, catalog) * REDOWNLOAD_SHARE) return 'far';
			catalog.stage(LIBRARY, JSON.stringify(page.changes));
			if (page.next >= page.head) {
				// The library feed writes no row a device write's undo holds (playlists, their items,
				// hidden tracks), so the device's own writes stay as they are through it.
				await this.#apply(id, catalog, LIBRARY, BigInt(page.next), stale, false);
				if (pulled > 0 && !stale()) this.#announce(id, told(kinds));
				return 'current';
			}
			after = page.next;
		}
	}

	/**
	 * Applies what `catalog` has staged for `feed` and moves its position to `through`, a slice a
	 * transaction, each sized to take about `SLICE_MS` and the worker answering between them: the
	 * reads of other libraries, the questions of state, and the other feeds' pulls. Until it ends,
	 * reads of the file go to the server or wait ([`onDevice`](#onDevice)). With `rebase`, the
	 * device's own writes are taken back first and made again after, as the log has them then
	 * ([`#rewind`](#rewind), [`#replay`](#replay)), so the server's rows land on the rows they were
	 * written over and a pending change is shown on top of them. Stops once `stale` says the file is
	 * not the one to write, leaving the file's mark for the next pull to finish. A slice that fails
	 * marks the file broken, so it answers nothing until an apply completes, and throws
	 * [`Unapplied`], which callers treat as damage; or, where the device is full, the error itself,
	 * reported as a quota error with the copy kept.
	 */
	async #apply(
		id: string,
		catalog: Catalog,
		feed: string,
		through: bigint,
		stale: () => boolean,
		rebase: boolean
	) {
		await this.#exclusive(catalog, async () => {
			let rewound: boolean;
			const fail = (error: unknown) => {
				this.#broken.add(catalog);
				this.#publish();
				// A full device leaves the file whole, if part-way: a later pull finishes it.
				if (describe(error).quota) return error;
				return new Unapplied(`staged changes would not apply: ${describe(error).message}`);
			};
			try {
				rewound = rebase && catalog.applied() > 0;
				// The feed brings what the server answered, so that is dropped, not kept to make again.
				if (rebase && !(await this.#rewind(catalog, stale, true))) return;
			} catch (error) {
				throw fail(error);
			}
			let limit = APPLY_CHUNK;
			while (!stale()) {
				let done: boolean;
				const started = performance.now();
				try {
					done = catalog.commit(feed, through, limit);
					limit = Math.max(APPLY_CHUNK, slice(limit, performance.now() - started, APPLY_MAX));
				} catch (error) {
					throw fail(error);
				}
				if (done) {
					// Readable again only once neither feed's changes are left part-way in it.
					if (catalog.applying(LIBRARY) === undefined && catalog.applying(ACCOUNT) === undefined)
						this.#broken.delete(catalog);
					if (rebase) await this.#replay(id, catalog, stale, await this.#pending(id), rewound);
					return;
				}
				// Told only once an apply takes more than a slice: one that is done at once is never seen.
				this.#publish();
				await new Promise((resolve) => setTimeout(resolve));
				await this.#queue;
			}
		});
	}

	/**
	 * Runs `job` on `catalog` after the apply under way, if any, keeping readers off the file until
	 * it ends: online reads go to the server, and offline ones wait ([`query`](#query)).
	 */
	async #exclusive(catalog: Catalog, job: () => Promise<void>) {
		const before = this.#applying.get(catalog);
		const run = (async () => {
			await before;
			await job();
		})();
		const end = run.catch(() => {});
		this.#applying.set(catalog, end);
		try {
			await run;
		} finally {
			if (this.#applying.get(catalog) === end) {
				this.#applying.delete(catalog);
				this.#publish();
			}
		}
	}

	/**
	 * Takes back every write the device made to `catalog` for the log, newest first, in slices of
	 * about `SLICE_MS`; those the server answered are kept to make again, unless `drop`. False if
	 * `stale` stopped it part-way.
	 */
	async #rewind(catalog: Catalog, stale: () => boolean, drop: boolean) {
		let limit = APPLY_CHUNK;
		for (;;) {
			const started = performance.now();
			if (catalog.rewind(limit, drop)) return true;
			limit = Math.max(APPLY_CHUNK, slice(limit, performance.now() - started, APPLY_MAX));
			await new Promise((resolve) => setTimeout(resolve));
			await this.#queue;
			if (stale()) return false;
		}
	}

	/**
	 * Makes again in `catalog` the writes the server answered that a rewind kept, then `pending`,
	 * the writes of the mutations the log holds for library `id`, in the order they were made, each
	 * at the time it was made, in slices of about `SLICE_MS`, each one transaction. Tells the page
	 * what they changed, every kind a write makes if writes were just taken back (`rewound`), which
	 * may not be made again. Stops once `stale` says the file is not the one to write.
	 */
	async #replay(
		id: string,
		catalog: Catalog,
		stale: () => boolean,
		pending: Call[],
		rewound = false
	) {
		const calls = [...(JSON.parse(catalog.answeredCalls()) as Call[]), ...pending];
		let made = 0;
		let limit = APPLY_CHUNK;
		for (let done = 0; done < calls.length;) {
			if (stale()) return;
			const started = performance.now();
			const batch = calls.slice(done, done + limit);
			made += this.#make(id, catalog, batch).made;
			done += batch.length;
			limit = Math.max(APPLY_CHUNK, slice(limit, performance.now() - started, APPLY_MAX));
			if (done < calls.length) {
				await new Promise((resolve) => setTimeout(resolve));
				await this.#queue;
			}
		}
		// What was taken back and not made again changed too, in any kind a write makes.
		if ((rewound || made > 0) && this.catalogs.get(id) === catalog)
			this.#changed(id, rewound ? REWOUND : this.#kinds(id, catalog, calls));
	}

	/**
	 * Makes those of `calls`, writes of mutations in library `id`, that `catalog` does not hold, in
	 * one transaction, and answers how many were made. A write the device refuses, as one made on
	 * a version the feed has since replaced, is not shown; the server's answer decides it. One that
	 * breaks a constraint, as a client-minted ID that is taken would, is that write's failure
	 * alone. With `answered`, the server has them: nothing is made, and those the file holds are
	 * kept for a rewind to make again until the account feed brings them.
	 */
	#make(id: string, catalog: Catalog, calls: Call[], answered = false) {
		const answer = JSON.parse(
			catalog.applyCalls(this.#user, id, JSON.stringify(calls), answered)
		) as { made: number; failed: string[] };
		for (const failure of answer.failed) console.error(failure);
		return answer;
	}

	/** The kinds of rows `calls` change in library `id`'s `catalog`, as a list depends on them. */
	#kinds(id: string, catalog: Catalog, calls: Call[]): Kind[] {
		const kinds = new Set<Kind>(['playlists']);
		const liked = catalog.liked(this.#user, id);
		for (const { name, args } of calls) {
			if (name === 'hide' || name === 'unhide') kinds.add('hidden');
			const playlist = (args as { playlistId?: string }).playlistId;
			if (name === 'like' || name === 'unlike' || name === 'hide' || playlist === liked)
				kinds.add('liked');
		}
		return [...kinds];
	}

	/**
	 * Makes those of `calls`, a mutation's writes to library `library`, that its file does not hold,
	 * after any apply under way, and tells the page what they changed. Nothing where the library
	 * is not on the device: they are made when it is ([`#replay`](#replay)). With `answered`, the
	 * server has answered the mutation, which has left the log or is about to: none is made, and
	 * those the file holds are made again through every rebase until an apply of the account feed,
	 * which brings them; one it lacks is left to that feed, which may have brought it already.
	 */
	async mutate(library: string, calls: Call[], answered = false) {
		for (;;) {
			await this.#queue;
			const catalog = this.catalogs.get(library);
			if (this.#closed || !catalog || this.#broken.has(catalog)) return;
			const applying = this.#applying.get(catalog);
			if (applying) {
				await applying;
				continue;
			}
			try {
				if (this.#make(library, catalog, calls, answered).made > 0)
					this.#changed(library, this.#kinds(library, catalog, calls));
			} catch (error) {
				void this.#run(`lib:${library}`, () => this.#rebuild(library));
				throw error;
			}
			return;
		}
	}

	/**
	 * Hears that the server answered a write to playlist `playlistId` of `library` with the playlist
	 * at `version`, once the write has left the log. If the device's copy is past that version and
	 * the log's writes to the playlist still to come, the feed brought the server's rows of the write
	 * while it was in the log, and the write was made again over them: the library is rebased, which
	 * makes it once. A write made on a version, such as an edit, is refused on such rows instead,
	 * so only an append needs this.
	 */
	async confirmed(library: string, playlistId: string, version: number) {
		for (;;) {
			await this.#queue;
			const catalog = this.catalogs.get(library);
			if (this.#closed || !catalog || this.#broken.has(catalog)) return;
			const applying = this.#applying.get(catalog);
			if (applying) {
				await applying;
				continue;
			}
			let held: number | undefined;
			try {
				const read = catalog.query(
					this.#user,
					library,
					'getPlaylist',
					JSON.stringify({ playlistId })
				);
				held = (JSON.parse(read) as { version?: number } | null)?.version;
			} catch {
				return;
			}
			const later = (await this.#pending(library)).filter(
				(call) => (call.args as { playlistId?: string }).playlistId === playlistId
			).length;
			if (held !== undefined && held > version + later) await this.rebase(library, true);
			return;
		}
	}

	/**
	 * Takes back every write the device made to `library` for the log and makes again those the
	 * server answered and those the log holds now, as after a mutation left the log unsent; readers
	 * are kept off the file meanwhile, as during an apply. With `drop`, those the server answered are
	 * not made again: the feed brought them.
	 */
	async rebase(library: string, drop = false) {
		await this.#queue;
		const catalog = this.catalogs.get(library);
		if (this.#closed || !catalog) return;
		const stale = () => this.#gone(library, catalog);
		try {
			await this.#exclusive(catalog, async () => {
				if (stale() || this.#broken.has(catalog)) return;
				// Read once the file is this job's, so a write asked for meanwhile waits and is made after.
				const pending = await this.#pending(library);
				const rewound = catalog.applied() > 0;
				// Nothing made and nothing to make, as at most starts.
				if (!rewound && pending.length === 0 && catalog.answeredCalls() === '[]') return;
				if (await this.#rewind(catalog, stale, drop))
					await this.#replay(library, catalog, stale, pending, rewound);
			});
		} catch (error) {
			if (damaged(error)) void this.#run(`lib:${library}`, () => this.#rebuild(library));
			this.#fail(error);
		}
	}

	/** Finishes the applies a closed worker left part-way in the library's file `catalog`. */
	async #finish(id: string, catalog: Catalog) {
		const stale = () => this.#gone(id, catalog);
		for (const feed of [LIBRARY, ACCOUNT]) {
			const through = stale() ? undefined : catalog.applying(feed);
			if (through !== undefined)
				await this.#apply(id, catalog, feed, through, stale, feed === ACCOUNT);
		}
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
		this.#publish();
		await this.#removeFiles(id);
	}

	/**
	 * Deletes a library the account no longer reaches, and tells `lost` so what else is kept for it
	 * goes too.
	 */
	async #lose(id: string) {
		await this.#discard(id);
		await this.#lost(id);
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
	 * it and the device's own writes over them, and swaps it in for any copy the library has,
	 * announcing the library as changed.
	 * Nothing shows until the swap. Undefined when the account does not reach the library. Unless
	 * `forced`, a snapshot no newer than the copy's own position is not imported: it is asked for
	 * again while the server builds a newer one, and the copy is returned if none comes.
	 */
	async #install(id: string, forced: boolean) {
		await this.#mine();
		const existing = this.catalogs.get(id);
		const generation = (await this.#names()).reduce((latest, name) => {
			const match = FILE.exec(name);
			return match?.[1] === id ? Math.max(latest, Number(match[2])) : latest;
		}, 0);
		const file = `lib-${id}-${generation + 1}.db`;
		// Every open file keeps its journal too, and the installs under way each add a new file.
		await this.#reserve(2 * (this.catalogs.size + this.#booting + 2));
		let seq: number;
		for (let mismatches = 0, older = 0; ;) {
			const response = await this.#snapshot(id);
			if (!response) {
				await this.#lose(id);
				return undefined;
			}
			seq = Number(response.headers.get('x-jewelcase-seq'));
			if (existing && !forced && seq <= Number(existing.cursor(LIBRARY))) {
				if (this.catalogs.get(id) === existing) {
					void response.body?.cancel();
					if (++older === BUILD_TRIES) return existing;
					// Asking for a stale snapshot starts the build of a newer one.
					await this.#sleep(this.#retryAfter(response, REBUILD_WAIT));
					continue;
				}
			}
			try {
				await this.#import(id, file, response);
				break;
			} catch (error) {
				const mismatch = error instanceof Error && error.message.includes('hash mismatch');
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
			catalog.commit(LIBRARY, BigInt(seq), APPLY_CHUNK);
			catalog.commit(OWNER, BigInt(this.#user), APPLY_CHUNK);
			// The device's own writes show on the new copy as they did on the old.
			await this.#replay(id, catalog, () => this.#closed, await this.#pending(id));
			if (this.#closed) throw new Closed();
		} catch (error) {
			catalog?.free();
			await this.#remove(file);
			await this.#remove(`${file}-journal`);
			throw error;
		}
		this.catalogs.get(id)?.free();
		this.catalogs.set(id, catalog);
		this.#publish();
		await this.#removeFiles(id, file);
		this.#announce(id, ALL);
		return catalog;
	}

	/**
	 * The snapshot's response, waiting out `503 catalog_building`; null where the account does
	 * not reach the library. A snapshot of another schema version than this build's stops the
	 * library's sync for good: only a reload, which brings the new build, helps.
	 */
	async #snapshot(id: string) {
		for (let tries = 0; tries < BUILD_TRIES; tries++) {
			const response = await this.#get(`/libraries/${id}/catalog`);
			if (response.status === 404) return null;
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
	 * Imports the snapshot of `response` into the pool as `file`. The body is first written to a
	 * plain OPFS file beside the pool, so the network never holds the pool, which a transfer
	 * takes whole; then one quick local pass copies it in, hashing it as it goes, and publishes
	 * it only if the hash is the ETag.
	 */
	async #import(id: string, file: string, response: Response) {
		const etag = response.headers.get('etag')?.replaceAll('"', '');
		if (!etag || !response.body) throw new Error('the catalog file has no ETag');
		const root = await navigator.storage.getDirectory();
		const name = DOWNLOAD(id);
		try {
			const handle = (await root.getFileHandle(name, { create: true })) as WorkerFile;
			const download = await handle.createSyncAccessHandle();
			try {
				const reader = response.body.getReader();
				let at = 0;
				for (;;) {
					const { done, value } = await reader.read().catch(() => {
						throw this.#closed ? new Closed() : new Offline();
					});
					if (done) break;
					if (download.write(value, { at }) !== value.length) throw new Error('the device is full');
					at += value.length;
				}
				download.flush();
				await this.#pool(async () => {
					const size = download.getSize();
					const target = await Import.begin(file, size);
					// The copy below holds the worker until it is done.
					this.#hold(true);
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
					} finally {
						this.#hold(false);
					}
					target.finish(etag);
				});
			} finally {
				download.close();
			}
		} finally {
			await root.removeEntry(name).catch(() => {});
		}
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

	/** Deletes the downloads a closed worker left behind. */
	async #clearDownloads() {
		const root = await navigator.storage.getDirectory();
		for await (const name of root.keys()) if (DOWNLOADS.test(name)) await root.removeEntry(name);
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

	/** Fetches `path` of the API. */
	async #get(path: string) {
		if (this.#closed) throw new Closed();
		try {
			return await this.#fetch(`${this.#api}${path}`, { signal: this.#abort.signal });
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
