// The account's queue on this device: the server's queue as confirmed, plus this device's own
// edits that the server has not yet confirmed, replayed on top (`design/queue.md` §9,
// `requirements/general.md` §3.4). Its starts, edits and restores are mutations of the page's one
// log (`$lib/sync/mutations/queue.ts`), which sends them in order; this applies each at once and
// hears what the server answered (`settled`). The order is the shared core's, run in
// WebAssembly, so this holds no rule about it, and it holds no tracks in its state. Tracks come
// from the lists, which it downloads by hash in the background and attaches to the core, and
// meanwhile from pages of rows (`design/queue.md` §3). Plain TypeScript so it runs in a worker
// and in tests; `worker.ts` is its shell.

import type { CatalogRouter } from '$lib/catalog/router';
import { Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type {
	AppliedEdit,
	PlayContext,
	QueueEdit,
	QueueRecipe,
	QueueSnapshot,
	RealtimeServerMessage,
	RestoredQueue,
	SentEdit,
	StartedQueue,
	TrackPage
} from '$lib/api/types';
import type { Mutation } from '$lib/sync/mutation';
import type { EditArgs, StartArgs } from '$lib/sync/mutations/queue';
import type { Result, Settled } from '$lib/sync/sender';
import { wipeChannel, type Env, type Port, type Stored } from '$lib/sync/store';
import { download, ListCache } from './lists';
import { PAGE, RowPages } from './rows';
import type {
	QueueMutation,
	QueueRow,
	Refusal,
	Repeat,
	RestoreResult,
	RowSection,
	RowsWindow,
	StartRequest,
	Summary
} from './protocol';

export type Options = {
	/** The API's base path, such as `/api/v1`. */
	api: string;
	/** The account signed in, whose files alone the catalog's owner answers the queue from. */
	user: string;
	fetch?: typeof fetch;
	/**
	 * Where its starts, edits and restores go to be sent: the page's log. What becomes of each is
	 * given back to `settled`.
	 */
	log: { enqueue(mutation: QueueMutation): void };
	/** Where lists are kept; the browser's Cache Storage if left out. Without any, memory. */
	storage?: CacheStorage;
	/**
	 * The way to the device's catalog, whichever tab owns it, for the queue to resolve lists with.
	 */
	catalog?: Pick<CatalogRouter, 'ask'>;
	/** Called after every change, with what the page shows. */
	changed: (summary: Summary) => void;
	/** Called when an edit could not be kept. */
	refused: (refusal: Refusal) => void;
	/**
	 * The user's database, where the queue the server confirmed is kept for the next time the app
	 * opens, and whose log holds what has not reached the server (`design/queue.md` §7); nothing is
	 * kept or read again if left out.
	 */
	database?: Env;
	/**
	 * Called with a plain notice for the user that is not about an edit, such as the queue not being
	 * kept.
	 */
	told?: (message: string) => void;
};

/** An edit this device made that the server has not confirmed. */
type Pending = {
	editId: string;
	queueId: string;
	edit: SentEdit;
	/** The edit as the core applies it here; null when only the server can (see `localEdit`). */
	local: QueueEdit | null;
	/** The version the server answered it made, once it has. */
	version?: number;
	/**
	 * The list this device resolved for an `add` of a recipe, once it has: JSON as a list's download
	 * gives it.
	 */
	device?: { hash: string; body: string };
	/**
	 * The count of rows an `add` of a recipe shows until the server says how many, as given to
	 * `edit`.
	 */
	length?: number;
	/**
	 * Shown again from the log as the app opened, and made by a tab that may still be open, so the
	 * server's queue, once fetched, says whether it has it.
	 */
	replayed?: boolean;
};

/** The key of the account's one record in the `queue` store. */
export const RECORD = 'queue';

/**
 * What the account's queue keeps in the database's `queue` store: the queue the server confirmed,
 * which the unsent starts and edits of the log go on top of when the app opens again. `createdAt`
 * is when it last changed: an unchanged queue is not written again.
 */
export type QueueRecord = Stored & {
	/** The confirmed queue's ID; null with no queue. */
	queueId: string | null;
	/** What the confirmed queue started from; null with no queue. */
	context: PlayContext | null;
	/** The confirmed queue's version. */
	version: number;
	/** The part of the server's `ETag` that is new each time it starts. */
	run: string | null;
	/** The confirmed queue's state, as the core writes it; null with no queue. */
	state: string | null;
};

/**
 * A restore this device asked for and the server has not answered: the `#epoch` it was asked at,
 * which a later start or restore moves on, and who waits for its answer, if anyone still does.
 */
type Restoring = { epoch: number; settle: ((result: RestoreResult) => void) | null };

/**
 * A start this device made that the server has not answered: the request, how long the view said
 * the source was, to tell whether the server's list is the view's, and the rows the view loaded
 * of a recipe's source. `replayed` as for `Pending`.
 */
type Starting = StartArgs & { replayed?: boolean };

/** What the state says of a source that rows need. */
type SourceInfo = { libraryId: string; list?: string };

/** A row as the core reads it. */
type CoreRow = {
	item: string;
	source: string;
	position: number;
	track: string | null;
	play?: number;
};

/** The rows of each part of the queue that hold a hidden track, by index, ascending. */
type Holding = { upNext: number[]; history: number[] };

/** How many hidden tracks a read of Hidden Songs asks for at once: the most the server gives. */
const HIDDEN_PAGE = 1000;

/** Why a list cannot be had: the server has no such list, or it could not be reached. */
type Problem = 'gone' | 'failed';

type QueueEditMessage = Extract<RealtimeServerMessage, { type: 'queueEdit' }>;

export class QueueStore {
	#api: string;
	#user: string;
	#fetch: typeof fetch;
	#log: Options['log'];
	#changed: (summary: Summary) => void;
	#refused: (refusal: Refusal) => void;

	/** This instance, as the `owner` its record is written with. */
	#instance = crypto.randomUUID();
	/** The user's database while this instance holds it; null with none or once closed. */
	#port: Port | null = null;
	/** Tells the user something that is not about an edit (`Options.told`). */
	#notice: (message: string) => void;
	/**
	 * The user's database is being deleted: nothing more is written to it, though writes already
	 * made are not undone.
	 */
	#wiped = false;
	/** What was last written says this of the queue; a change of it is what is worth writing. */
	#savedSignature = '';
	/** Every write, in order, so none overtakes another. */
	#tail: Promise<void> = Promise.resolve();
	/** Stops hearing that the database is being wiped. */
	#unwipe: (() => void) | null = null;
	/** Whether the user has been told that this browser would not keep the queue. */
	#unkeptTold = false;
	/**
	 * The starts, edits and restores this instance gave the log that are not settled, as
	 * `start:`, `edit:` or `restore:` and the ID: the queue is fetched only once there are none,
	 * so what it fetches holds them, as when the queue sent its own.
	 */
	#own = new Set<string>();
	/**
	 * The starts, edits and restores the log held as the app opened, as `#own` names them: the
	 * queue is not fetched while any is still in the log, whichever tab sends it, so a queue shown
	 * from the log is not replaced by the one it replaced.
	 */
	#replayed = new Set<string>();
	/** A drain was asked for while one ran, which looks again once it is done. */
	#drainAgain = false;
	/** Counts the times the server was heard from, so a fetch that failed before is tried again. */
	#reached = 0;
	/** The `#reached` a fetch of the queue failed at: it is tried again only once that moves on. */
	#failedAt = -1;

	/** The server's queue: its snapshot with every edit since applied, in version order. */
	#confirmed: Queue | null = null;
	/** What this device shows: `#confirmed` with `#pending` on top. The same object with no pending. */
	#view: Queue | null = null;
	#queueId: string | null = null;
	/** What the queue started from, as the server keeps it; null with no queue. */
	#context: PlayContext | null = null;
	#version = 0;
	/** The part of the server's `ETag` that is new each time it starts, as last given. */
	#run: string | null = null;
	#revision = 0;
	#pending: Pending[] = [];
	#refetch = false;
	/** Edits heard while the queue is being fetched; the fetch may predate them. */
	#held: QueueEditMessage[] | null = null;
	#draining = false;
	/** The start of the queue shown, until the server answers it. */
	#starting: Starting | null = null;
	/** The restores this device asked for that the server has not answered, by kept queue. */
	#restores = new Map<string, Restoring>();
	/** The rows of the view a start came from, by position, for the source it started (`rows`). */
	#viewRows = new Map<string, string[]>();
	/** The item a recipe start played, until the server's snapshot says whether it is the one it made current. */
	#startItem: string | null = null;
	/** Counts starts, so a fetch that began before one is not installed over it. */
	#epoch = 0;
	/** The context's rows are in a shuffled order, which a view's rows cannot fill. */
	#shuffled = false;
	/** The item playing, which the shuffle does not move. */
	#current: string | null = null;

	#lists: ListCache;
	#pages: RowPages;
	/** The lists attached to the confirmed queue, by hash. */
	#attached = new Set<string>();
	/** The hashes either queue references, as of the last change, to tell what clean-up dropped. */
	#referenced = new Set<string>();
	/** The lists that cannot be had, and are not asked for again: `failed` ones when the connection is back. */
	#problems = new Map<string, Problem>();
	/** What the view's state says of each source, by key. */
	#sources = new Map<string, SourceInfo>();
	#loading = false;
	/** The way to the device's catalog, for resolving lists from its files. */
	readonly catalog?: Pick<CatalogRouter, 'ask'>;

	/**
	 * The tracks the user hid, by library (`requirements/conventions.md` §8). Their rows stay in
	 * the queue, which no hide edits, and are left out of the rows and counts the page is given,
	 * so the player passes over them as they come up. Read from Hidden Songs for each library a
	 * source plays from, again on reconnecting, and kept up by what the page says changed.
	 */
	#hidden = new Map<string, Set<string>>();
	/** The libraries whose hidden tracks were asked for since the connection said hello. */
	#hiddenAsked = new Set<string>();
	/**
	 * What the page said changed since the connection said hello, by library: a read of Hidden
	 * Songs may predate any of it, or miss a change still on its way to the server.
	 */
	#told = new Map<string, Map<string, boolean>>();
	/**
	 * Where each attached list holds a hidden track, by hash. A list never changes, so this is
	 * found once for each and holds until the hidden tracks change; a change to the queue then
	 * costs what these number.
	 */
	#hits = new Map<string, number[]>();
	/** The rows that hold a hidden track, as of the last change; rows not yet read hold none. */
	#holding: Holding = { upNext: [], history: [] };

	/** Makes an empty store; it loads the server's queue when the connection says hello. */
	constructor({
		api,
		user,
		fetch: fetcher = (...args) => fetch(...args),
		log,
		storage = globalThis.caches,
		catalog,
		changed,
		refused,
		database,
		told = () => {}
	}: Options) {
		this.catalog = catalog;
		this.#api = api;
		this.#user = user;
		this.#fetch = fetcher;
		this.#log = log;
		this.#changed = changed;
		this.#refused = refused;
		this.#notice = told;
		if (database) {
			this.#port = database.open(user);
			const channel = wipeChannel(user);
			if (channel) {
				channel.onmessage = () => this.#wipe();
				this.#unwipe = () => channel.close();
			}
		}
		this.#lists = new ListCache(storage, api, user);
		this.#pages = new RowPages(
			(path) => this.#get(path),
			() => this.#emit()
		);
	}

	/**
	 * Up to `limit` rows of `section` from `offset`, as the page shows them (`design/queue.md`
	 * §3). Each row's track is the list's once attached, else a page's, else it is `unknown` while
	 * a list or page that holds it is on its way, and `unavailable` if none is: a list not yet
	 * confirmed is always on its way, however the server answers, and one that could not be reached
	 * only while a page for the row is. Rows for which no list is attached yet start pages for the
	 * window, asked for again only when the version changes, so asking again when `revision`
	 * changes never loops.
	 */
	rows({
		section,
		offset,
		limit
	}: {
		section: RowSection;
		offset: number;
		limit: number;
	}): RowsWindow {
		const view = this.#view;
		if (!view) return { section, offset, total: 0, revision: this.#revision, rows: [] };
		// Read past the rows that hold a hidden track, which the page is not shown: `at` is where
		// each row shown is in the whole of its part.
		const skip = section === 'current' ? [] : this.#holding[section];
		const from = section === 'current' ? 0 : unskipped(offset, skip);
		const left = new Set(skip);
		const read = this.#coreRows(view, section, from, limit + skip.length)
			.map((row, i) => ({ row, at: from + i }))
			.filter(({ at }) => !left.has(at))
			.slice(0, limit);
		const raw = read.map(({ row }) => row);
		const total =
			section === 'upNext'
				? view.upNextLen() - skip.length
				: section === 'history'
					? view.historyLen() - skip.length
					: raw.length;
		if (raw.some((row) => this.#needsPage(row))) {
			this.#ensurePages(section, from, (read.at(-1)?.at ?? from) - from + 1);
		}
		// The part whose pages hold the window's rows: the item playing comes with Up Next's first.
		const part = section === 'current' ? 'upNext' : section;
		const rows = raw.map((row, i): QueueRow => {
			const { libraryId, list } = this.#sources.get(row.source) ?? { libraryId: '' };
			// The list, else a page, else the tracks of an add of this device's own that waits.
			const held = row.track !== null ? row.track : this.#fromPageOrOwn(row);
			if (row.track !== null) this.#pages.forget(row.item);
			// A list not yet confirmed, or not given up on, is on its way, and so is a page for it.
			const problem = list === undefined ? undefined : this.#problems.get(list);
			const status =
				typeof held === 'string'
					? 'known'
					: held === null ||
						  problem === 'gone' ||
						  (problem === 'failed' && !this.#pages.fetching(part, read[i].at))
						? 'unavailable'
						: 'unknown';
			const out: QueueRow = {
				item: row.item,
				libraryId,
				status,
				track: typeof held === 'string' ? held : null
			};
			if (row.play !== undefined) out.play = row.play;
			return out;
		});
		return { section, offset, total, revision: this.#revision, rows };
	}

	/**
	 * A page's track for the row, null if the server could not read it, else a pending add's,
	 * else the view's the queue started from, else undefined. The view's fill only rows of an
	 * unshuffled queue, which read straight from the list.
	 */
	#fromPageOrOwn(row: CoreRow): string | null | undefined {
		const page = this.#pages.track(row.item);
		if (page !== undefined) return page;
		const straight = !this.#shuffled || row.item === this.#current;
		return (
			this.#ownTrack(row) ?? (straight ? this.#viewRows.get(row.source)?.[row.position] : undefined)
		);
	}

	/**
	 * The track a pending add of `tracks` gave the row, by its position: the server's list is
	 * those tracks, so this is right through a copy or the shuffle too; undefined if none.
	 */
	#ownTrack(row: CoreRow): string | undefined {
		for (const { edit } of this.#pending) {
			if (edit.type === 'add' && edit.source.key === row.source) {
				return edit.source.tracks?.[row.position];
			}
		}
		return undefined;
	}

	/** The rows of `view` the core reads for a window. */
	#coreRows(view: Queue, section: RowSection, offset: number, limit: number): CoreRow[] {
		if (section === 'current') {
			const row = view.currentRow();
			return row ? [JSON.parse(row)] : [];
		}
		return JSON.parse(
			section === 'upNext' ? view.upNextRows(offset, limit) : view.historyRows(offset, limit)
		);
	}

	/** Whether a page could tell the row's track: no list has it, and no page or problem has said. */
	#needsPage(row: CoreRow): boolean {
		const list = this.#sources.get(row.source)?.list;
		return (
			row.track === null &&
			list !== undefined &&
			this.#problems.get(list) !== 'gone' &&
			this.#pages.track(row.item) === undefined
		);
	}

	/** Starts the pages for a window; the item playing is in Up Next's first. */
	#ensurePages(section: RowSection, offset: number, limit: number) {
		if (!this.#queueId) return;
		const [part, from, rows]: [Exclude<RowSection, 'current'>, number, number] =
			section === 'current' ? ['upNext', 0, 1] : [section, offset, limit];
		this.#pages.ensure(this.#queueId, this.#version, part, from, rows);
	}

	/**
	 * One request to the API, with `headers` besides the client's; null when it could not be made.
	 * Nothing tries it again: what failed is asked for once the server is heard from.
	 */
	async #get(path: string, headers: Record<string, string> = {}): Promise<Response | null> {
		return this.#fetch(`${this.#api}${path}`, {
			headers: { 'X-Jewelcase-Client': 'web', ...headers }
		}).catch(() => null);
	}

	/**
	 * Starts a queue here at once, as a start from a view does (`design/queue.md` §3): the
	 * queue is the new one, its source as long as the view says and with no list, the clicked row
	 * playing, and the start is given to the log, which sends it after everything made before it.
	 * The edits made on the queue it replaces are not dropped: they are sent first, so the server
	 * keeps that queue with them, as when they were sent before the start. The view's own rows fill
	 * the unshuffled rows they cover. Answers false, changing nothing, if the start is not a
	 * position of the source. The new queue keeps the repeat mode shown, as the server's takes the
	 * active queue's (`requirements/queue.md` §5), which any repeat change unsent goes to first.
	 * A start of a recipe is also resolved on the device (see `#attachResolved`); `offline` says
	 * whether the app counts itself offline, and is not sent to the server.
	 */
	start({ total, loaded, offline, ...body }: StartRequest): boolean {
		const args: StartArgs = { body, total, loaded };
		if (!this.#begin(args, !!offline)) return false;
		this.#enqueue({ name: 'startQueue', args });
		this.#emit();
		return true;
	}

	/**
	 * Makes the start of `args` the queue shown, waiting for the server's answer; false, changing
	 * nothing, if the start is not a position of its source. `replayed` for one shown again from the
	 * log as the app opens.
	 */
	#begin(args: StartArgs, offline: boolean, replayed = false): boolean {
		const { body, total, loaded } = args;
		const { key, libraryId, tracks } = body.source;
		const length = tracks?.length ?? total ?? 0;
		// The core makes the state; it rounds what it is given, so a position it would not is refused here.
		if (!Number.isInteger(body.start) || body.start < 0) return false;
		let next: Queue;
		try {
			const repeat = this.#view ? JSON.parse(this.#view.toJson()).repeat : 'off';
			next = Queue.started(key, libraryId, length, body.start, repeat);
		} catch {
			return false;
		}
		this.#epoch++;
		this.#startItem = null;
		this.#release();
		this.#confirmed = this.#view = next;
		this.#queueId = body.queueId;
		this.#context = body.context;
		this.#version = 0;
		this.#run = null;
		this.#refetch = false;
		this.#attached.clear();
		this.#problems.clear();
		this.#pages.reset(body.queueId);
		this.#viewRows.set(key, tracks ?? loaded ?? []);
		// The edits of the queue it replaces are not shown on the new one; the log still sends them.
		this.#pending = [];
		this.#starting = { ...args, replayed };
		if (body.source.recipe) void this.#attachResolved(key, libraryId, body.source.recipe, offline);
		return true;
	}

	/** Gives `mutation` to the log, as this instance's own until it is settled. */
	#enqueue(mutation: QueueMutation) {
		this.#own.add(idOf(mutation));
		this.#log.enqueue(mutation);
	}

	/**
	 * The recipe's list as the device's catalog resolves it, hashed by the core; null when it
	 * cannot (no owner, the library is not on the device, a query error, a playlist that is not
	 * the user's); the queue then goes on as it does without a catalog. A search is left to the
	 * server unless the page counts itself `offline`: its ranking is the server's, and the
	 * device's index costs memory to build.
	 */
	async #resolve(
		libraryId: string,
		recipe: QueueRecipe,
		offline: boolean
	): Promise<{ hash: string; ids: string[] } | null> {
		if (!this.catalog || (recipe.type === 'search' && !offline)) return null;
		try {
			const params = { recipe };
			const request = {
				type: 'query',
				library: libraryId,
				name: 'resolveRecipe',
				params,
				offline,
				user: this.#user
			} as const;
			return JSON.parse(await this.catalog.ask(request));
		} catch {
			return null;
		}
	}

	/**
	 * Gives a started recipe's source the list the device resolves for it, without waiting for the
	 * server: attached under the device's hash while the server has named none, and kept in storage.
	 * Once the server names the list, the device's is used only if it names the same hash, so a
	 * snapshot that agrees downloads nothing and one that does not downloads its own. Does nothing
	 * if the device could not resolve it or the queue no longer has the source: another start has
	 * a new key, and a queue that names a list takes the device's only under the same hash.
	 */
	async #attachResolved(key: string, libraryId: string, recipe: QueueRecipe, offline: boolean) {
		const found = await this.#resolve(libraryId, recipe, offline);
		const confirmed = this.#confirmed;
		if (!found || !confirmed) return;
		const state: { sources: { key: string; list?: string }[] } = JSON.parse(confirmed.toJson());
		const source = state.sources.find((source) => source.key === key);
		const { hash, ids } = found;
		if (
			!source ||
			(source.list !== undefined && (source.list !== hash || this.#attached.has(hash)))
		) {
			return;
		}
		const body = JSON.stringify({ libraryId, tracks: ids });
		try {
			if (source.list === undefined) {
				confirmed.attachSource(key, hash, body);
				this.#attached.add(hash);
				this.#viewRows.delete(key);
			} else if (!this.#attach(hash, body)) {
				return;
			}
		} catch {
			// Not the length the view said: the server's list is fetched as without a catalog.
			return;
		}
		void this.#lists.put(hash, body);
		this.#rebuild();
	}

	/**
	 * Resolves a pending `add` of a recipe on the device and shows its rows from that list at
	 * once, as an add of `tracks` does; the list is kept for the server's confirmation if the
	 * server names the same hash. Does nothing once the edit is no longer pending.
	 */
	async #resolveAdd(pending: Pending, libraryId: string, recipe: QueueRecipe, offline: boolean) {
		const found = await this.#resolve(libraryId, recipe, offline);
		if (!found || !this.#pending.includes(pending)) return;
		pending.device = { hash: found.hash, body: JSON.stringify({ libraryId, tracks: found.ids }) };
		this.#rebuild();
	}

	/**
	 * Makes the kept queue `queueId` the account's active one (`POST /me/queues/{id}/restore`),
	 * given to the log after what was made before it. Nothing shows until the server answers, since
	 * only it holds the queue; the answer is then installed as a fetch's is, unless a start or
	 * restore made since replaced it. Settles with where playback was left, or why the queue was not
	 * restored; one restore at a time. Kept queues are the server's alone until devices keep them,
	 * so with `offline` (the app counts itself offline) nothing is logged and it settles at once as
	 * `offline`; one the page takes back from the log because the server could not be reached
	 * settles as `unreachable` (`client.svelte.ts`).
	 */
	restore(queueId: string, offline = false): Promise<RestoreResult> {
		if (offline) {
			const message = 'Kept queues can be restored once you’re back online.';
			return Promise.resolve({ ok: false, code: 'offline', message });
		}
		if (this.#restores.size > 0) {
			return Promise.resolve({ ok: false, code: 'busy', message: 'Another restore is under way.' });
		}
		// A fetch under way is of the queue this replaces.
		const restoring: Restoring = { epoch: ++this.#epoch, settle: null };
		this.#restores.set(queueId, restoring);
		this.#enqueue({ name: 'restoreQueue', args: { queueId } });
		return new Promise((settle) => (restoring.settle = settle));
	}

	/**
	 * Applies `edit` here at once and gives it to the log with a fresh `editId`, which the server
	 * applies once however many times it is sent. An edit the core refuses changes nothing and is
	 * reported. An `add` of a `recipe` applies here as `length` rows, the count of the view it
	 * comes from, which wait for the server's list or pages (`design/queue.md` §3); the server's
	 * `queueEdit` gives the length it resolved. Without a length it is just sent, and shows when
	 * that arrives. An `add` of a recipe is also resolved on the device (see `#resolveAdd`);
	 * `offline` says whether the app counts itself offline, which lets a search be resolved there
	 * too.
	 */
	edit(edit: SentEdit, length?: number, offline = false) {
		if (!this.#view || !this.#queueId) {
			this.#refused({ code: 'no_queue', message: 'There is no queue to change.', edit });
			return;
		}
		const local = localEdit(edit, length);
		if (local) {
			const next = this.#view === this.#confirmed ? this.#view.copy() : this.#view;
			const code = applyTo(next, local);
			if (code) {
				if (next !== this.#view) next.free();
				this.#refused({ code, message: `The queue refused this change (${code}).`, edit });
				return;
			}
			this.#view = next;
		}
		const args: EditArgs = { editId: crypto.randomUUID(), queueId: this.#queueId, edit, length };
		this.#pend(args, local, offline);
		this.#enqueue({ name: 'editQueue', args });
		this.#emit();
	}

	/**
	 * Holds the edit of `args`, made here as `local`, as pending on the queue shown, and resolves an
	 * `add` of a recipe on the device.
	 */
	#pend(args: EditArgs, local: QueueEdit | null, offline: boolean, replayed = false) {
		const { editId, queueId, edit } = args;
		const pending: Pending = { editId, queueId, edit, local, length: args.length, replayed };
		this.#pending.push(pending);
		if (edit.type === 'add' && edit.source.recipe) {
			void this.#resolveAdd(pending, edit.source.libraryId, edit.source.recipe, offline);
		}
	}

	// ── What the server answered ───────────────────────────────

	/**
	 * Takes in what became of a start, edit or restore the log held, as the page's sender heard it:
	 * sent, with the server's answer, or dropped, with why. One this instance no longer shows, as
	 * the start of a queue another start replaced, changes nothing here.
	 */
	settled(mutation: QueueMutation, outcome: Settled, result: Result) {
		if (outcome === 'sent') this.#reached++;
		const id = idOf(mutation);
		this.#own.delete(id);
		this.#replayed.delete(id);
		if (mutation.name === 'startQueue') {
			this.#started(mutation.args, outcome, result);
		} else if (mutation.name === 'editQueue') {
			this.#edited(mutation.args, outcome, result);
		} else {
			void this.#restored(mutation.args.queueId, outcome, result).finally(() => this.#drain());
			return;
		}
		this.#drain();
	}

	/**
	 * A start answered. Its answer gives the list's hash: a start of `tracks` attaches them under it,
	 * so they are never downloaded back, and a recipe's start fetches the queue, since the server's
	 * list and start may differ from the view's. A start the server refuses is gone, with the edits
	 * made on it, and the queue the server has is fetched.
	 */
	#started(args: StartArgs, outcome: Settled, result: Result) {
		const { body } = args;
		const starting = this.#starting;
		if (!starting || starting.body.queueId !== body.queueId) return;
		this.#starting = null;
		if (outcome !== 'sent') {
			this.#viewRows.clear();
			this.#pending = this.#pending.filter((pending) => pending.queueId !== body.queueId);
			this.#refetch = true;
			const { code = 'failed', message = '', dead } = result;
			// One set aside was told by the log.
			if (!dead) this.#refused({ code, message, edit: null });
			return;
		}
		const { list, length } = result.answer as StartedQueue;
		this.#emit();
		const { key, libraryId, tracks } = body.source;
		if (tracks) {
			const text = JSON.stringify({ libraryId, tracks });
			void this.#lists.put(list, text);
			try {
				this.#confirmed?.attachSource(key, list, text);
				this.#attached.add(list);
				this.#viewRows.delete(key);
				this.#rebuild();
			} catch {
				// The queue no longer has the source: its own list is fetched like any other.
			}
		} else {
			// The server's list, or its start, may differ from the view's: if it does, the view's rows
			// are no guide to the rows the snapshot names.
			if (length !== starting.total) this.#viewRows.delete(key);
			this.#startItem = `${key}.${body.start}`;
			this.#refetch = true;
		}
	}

	/**
	 * An edit answered: one sent has the version it made, and its add's list is kept under the hash
	 * the server named, so a snapshot that has the add never downloads it back. One refused is taken
	 * back and reported: `superseded` when another queue was active by then, which is then fetched.
	 */
	#edited({ editId }: EditArgs, outcome: Settled, result: Result) {
		const pending = this.#pending.find((pending) => pending.editId === editId);
		if (!pending) return;
		if (outcome === 'sent') {
			const applied = result.answer as AppliedEdit;
			pending.version = applied.version;
			const body = ownBody(pending, applied.source?.list);
			if (applied.source?.list && body) void this.#lists.put(applied.source.list, body);
			return;
		}
		this.#pending = this.#pending.filter((other) => other !== pending);
		const { code = 'failed', message = '', dead } = result;
		const replaced = code === 'queue_changed';
		if (replaced || code === 'not_found') this.#refetch = true;
		// One set aside was told by the log.
		if (!dead) this.#drop(pending, replaced ? 'superseded' : code, message);
		this.#rebuild();
	}

	/**
	 * A restore answered: installed as a fetch's answer is, unless a start or restore made since it
	 * was asked for replaced it, and settled for who waits. One taken back from the log is
	 * `unreachable`. One this instance did not ask for, as one a closed tab left, has the queue
	 * fetched.
	 */
	async #restored(queueId: string, outcome: Settled, result: Result) {
		const restoring = this.#restores.get(queueId);
		this.#restores.delete(queueId);
		const settle = restoring?.settle ?? (() => {});
		if (outcome === 'cancelled') {
			this.#refetch = true;
			return settle({
				ok: false,
				code: 'unreachable',
				message: 'The server could not be reached.'
			});
		}
		if (outcome !== 'sent') {
			// Whether another queue is active now is the server's to say.
			this.#refetch = true;
			const { code = 'failed', message = 'That queue wasn’t restored.' } = result;
			return settle({ ok: false, code, message });
		}
		if (!restoring) {
			this.#refetch = true;
			return;
		}
		if (restoring.epoch !== this.#epoch) {
			return settle({ ok: false, code: 'superseded', message: 'Another queue replaced this one.' });
		}
		const { queue, run } = result.answer as { queue: RestoredQueue; run: string | null };
		const epoch = ++this.#epoch;
		this.#run = run ?? this.#run;
		this.#startItem = null;
		this.#viewRows.clear();
		if (!(await this.#install(queue, epoch))) {
			return settle({ ok: false, code: 'superseded', message: 'Another queue replaced this one.' });
		}
		// An edit heard before this was installed named another queue, so a fetch follows for it.
		this.#caughtUp([]);
		settle({ ok: true, position: queue.position ?? null });
	}

	// ── Kept for the next time the app opens ───────────────────────────

	/**
	 * Shows the queue as the app left it: the queue the account's record holds, which the server
	 * confirmed, with the starts and edits of the log not yet sent on top, in the order they were
	 * made, whichever tab made them (`#replay`). The server's queue replaces it as soon as it is
	 * heard. Call it once, before the queue is used. Does nothing with no database, and one that
	 * cannot be read leaves the queue to load from the server.
	 */
	async resume() {
		const port = this.#port;
		if (!port) return;
		try {
			const [records, log] = await Promise.all([
				port.all<QueueRecord>('queue'),
				port.all<Mutation>('mutations')
			]);
			const record = records.find((record) => record.key === RECORD);
			if (record?.state) await this.#show(record);
			for (const mutation of log) {
				const id = idOf(mutation as unknown as QueueMutation);
				if (id) this.#replayed.add(id);
			}
			this.#replay(log.sort((a, b) => a.seq - b.seq));
			this.#rebuild();
			// Any other record is one kept for an instance, which the account's one record replaces.
			for (const { key, rev } of records)
				if (key !== RECORD) await port.remove('queue', key, rev).catch(console.error);
		} catch (error) {
			console.error(error);
		}
	}

	/** Makes the record's queue the confirmed one. */
	async #show({ queueId, version, context, state, run }: QueueRecord) {
		const snapshot = { queueId, version, context, state: JSON.parse(state!) } as QueueSnapshot;
		if (await this.#install(snapshot, this.#epoch)) this.#run = run;
	}

	/**
	 * Shows the starts and edits of the queue in `log`, in order, on top of the queue shown: a start
	 * replaces it unless it is that queue, which the server then has, and an edit goes on the queue
	 * it was made on, if that is the one shown. What a restore makes active only the server holds,
	 * so the edits made on it show once it is fetched. Each stays in the log, to be sent by this tab
	 * or another, and the queue is fetched only once all have left it (`#replayed`).
	 */
	#replay(log: Mutation[]) {
		for (const { name, args } of log) {
			if (name === 'startQueue') {
				const start = args as StartArgs;
				if (start.body.queueId !== this.#queueId) this.#begin(start, false, true);
			} else if (name === 'editQueue') {
				const edit = args as EditArgs;
				if (edit.queueId !== this.#queueId) continue;
				this.#pend(edit, localEdit(edit.edit, edit.length), false, true);
			}
		}
	}

	/**
	 * Writes the queue the server confirmed to the database now, after any write already begun, and
	 * settles once it is written or has failed. What the server has not answered is the log's to
	 * keep, so a queue started here is not written until its start is answered. A queue that has not
	 * changed since it was last written is not written again, so the record's time is that of its
	 * last change. Nothing is written once the database is being wiped.
	 */
	async save() {
		const port = this.#port;
		if (!port) return;
		if (this.#starting || this.#signature() === this.#savedSignature)
			return void (await this.#tail);
		const record: QueueRecord = {
			key: RECORD,
			rev: crypto.randomUUID(),
			owner: this.#instance,
			createdAt: Date.now(),
			tries: 0,
			queueId: this.#queueId,
			context: this.#context,
			version: this.#version,
			run: this.#run,
			state: this.#confirmed?.toJson() ?? null
		};
		this.#savedSignature = this.#signature();
		this.#tail = this.#tail
			.then(async () => {
				if (this.#wiped) return;
				await port.put('queue', record);
			})
			.catch((error) => this.#unkept(error));
		await this.#tail;
	}

	/**
	 * The user's database is being deleted, as the account's session ended: nothing more is written,
	 * and nothing more is fetched, since the next request may carry another account's session. A
	 * restore waited for is settled. It does nothing more until it is replaced.
	 */
	#wipe() {
		this.#wiped = true;
		this.close();
		this.#pending = [];
		this.#starting = null;
		this.#refetch = false;
		this.#own.clear();
		this.#replayed.clear();
		for (const { settle } of this.#restores.values())
			settle?.({ ok: false, code: 'unreachable', message: 'The session ended.' });
		this.#restores.clear();
	}

	/** Lets go of the database: nothing more is written once writes already begun are done. */
	close() {
		this.#unwipe?.();
		this.#unwipe = null;
		const port = this.#port;
		this.#port = null;
		if (port) void this.#tail.then(() => port.close());
	}

	/**
	 * What the queue is worth writing for: a change of this is a change of what a later instance
	 * would show.
	 */
	#signature(): string {
		const lists = this.#confirmed ? hashesOf(this.#confirmed).join() : '';
		return [this.#queueId, this.#version, this.#run, lists].join('|');
	}

	/**
	 * Writes the queue at once if it changed in a way a later instance would show; a change of rows
	 * alone writes nothing.
	 */
	#changedForSave() {
		if (this.#port && this.#signature() !== this.#savedSignature) void this.save();
	}

	/**
	 * Tells the user, once, that this browser would not keep the queue; not as a refused edit, which
	 * playback would answer to.
	 */
	#unkept(error: unknown) {
		console.error(error);
		if (this.#unkeptTold) return;
		this.#unkeptTold = true;
		this.#notice(
			'This browser couldn’t keep your queue for later, so it is kept only while the app is open.'
		);
	}

	/** Takes in what the server said over the realtime connection. */
	message(message: RealtimeServerMessage) {
		if (message.type === 'hello') {
			// What was hidden while the connection was down is read again, by the sync below: a
			// queue that did not change answers its fetch with 304, and nothing else would.
			this.#hiddenAsked.clear();
			this.#told.clear();
			// What could not be reached is asked for again.
			for (const [hash, problem] of this.#problems) {
				if (problem === 'failed') this.#problems.delete(hash);
			}
			this.#pages.retry();
			this.#reached++;
			this.#refetch = true;
			this.#drain();
			this.#sync();
		} else if (message.type === 'queueReplaced') {
			// A fetch under way may answer with the queue this replaces, so it is fetched again. One
			// shown from the log is fetched too: another tab's start of it may be what was heard.
			if (this.#held || message.queueId !== this.#queueId || this.#replayed.size > 0) {
				this.#refetch = true;
				this.#drain();
			}
		} else if (message.type === 'queueEdit') {
			this.#heard(message);
		}
	}

	/**
	 * Takes in that the user hid track `trackId` of `libraryId`, or unhid it, here or on another
	 * device, which changes the rows the page is shown at once.
	 */
	hidden(libraryId: string, trackId: string, hidden: boolean) {
		let told = this.#told.get(libraryId);
		if (!told) this.#told.set(libraryId, (told = new Map()));
		told.set(trackId, hidden);
		let tracks = this.#hidden.get(libraryId);
		if (!tracks) this.#hidden.set(libraryId, (tracks = new Set()));
		if (tracks.has(trackId) === hidden) return;
		if (hidden) tracks.add(trackId);
		else tracks.delete(trackId);
		this.#hits.clear();
		this.#emit();
	}

	/**
	 * Reads Hidden Songs for `libraryId` in the background, a page at a time, then takes it as
	 * the library's hidden tracks with what the page told since on top. One that cannot be read
	 * is asked for again only once the connection says hello.
	 */
	async #readHidden(libraryId: string) {
		this.#hiddenAsked.add(libraryId);
		try {
			const found = new Set<string>();
			let cursor: string | null | undefined = null;
			do {
				const after = cursor ? `&cursor=${encodeURIComponent(cursor)}` : '';
				const path = `/libraries/${encodeURIComponent(libraryId)}/hidden-tracks`;
				const response = await this.#get(`${path}?limit=${HIDDEN_PAGE}${after}`);
				if (!response?.ok) return;
				const page = (await response.json()) as TrackPage;
				for (const track of page.items) found.add(track.id);
				cursor = page.nextCursor;
			} while (cursor);
			for (const [trackId, hidden] of this.#told.get(libraryId) ?? []) {
				if (hidden) found.add(trackId);
				else found.delete(trackId);
			}
			const before = this.#hidden.get(libraryId) ?? new Set();
			this.#hidden.set(libraryId, found);
			if (found.size !== before.size || [...found].some((id) => !before.has(id))) {
				this.#hits.clear();
				this.#emit();
			}
		} catch {
			// Read again once the connection is back.
		}
	}

	/** An edit another device (or this one) made, as the server applied it. */
	#heard(message: QueueEditMessage) {
		if (this.#held) {
			this.#held.push(message);
			return;
		}
		if (message.queueId !== this.#queueId || !this.#confirmed) {
			this.#refetch = true;
		} else if (message.version === this.#version + 1) {
			this.#confirm(this.#confirmed, message);
		} else if (message.version > this.#version) {
			this.#refetch = true;
		}
		this.#drain();
	}

	/**
	 * Applies the next edit to the confirmed queue; ours, if it is, stops being pending, so it
	 * is never replayed over a queue that has it.
	 */
	#confirm(confirmed: Queue, { edit, editId, version }: QueueEditMessage) {
		const own = this.#pending.find((pending) => pending.editId === editId);
		const mine = own && edit.type === 'add' && edit.source.list ? edit.source.list : null;
		const body = own && mine ? ownBody(own, mine) : null;
		if (applyTo(confirmed, edit) !== undefined) {
			// The server applied what the core refuses: this device cannot trust its copy.
			this.#refetch = true;
			return;
		}
		this.#version = version;
		// Its tracks are held, so the rows need no page and the list no download.
		if (mine && body) this.#attach(mine, body);
		this.#pending = this.#pending.filter((pending) => pending.editId !== editId);
		this.#rebuild();
	}

	/**
	 * Makes the view from the confirmed queue and the pending edits. A pending edit the queue
	 * no longer accepts, because of what arrived, is dropped and reported with the core's code,
	 * `anchor_gone` when the item it was placed by went. A pending `add` of a `recipe` with no
	 * length stays pending without showing: the server applies it in the order it heard it, and
	 * so do the edits replayed on top of it once it arrives. With none pending the view is the
	 * confirmed queue itself, so nothing is copied.
	 */
	#rebuild() {
		const confirmed = this.#confirmed;
		if (!confirmed) return;
		let view = confirmed;
		if (this.#pending.length > 0) {
			view = confirmed.copy();
			this.#pending = this.#pending.filter((pending) => {
				const code = pending.local && applyTo(view, pending.local);
				if (code) this.#drop(pending, code, `The queue refused this change (${code}).`);
				else if (pending.device && pending.edit.type === 'add') {
					try {
						view.attachSource(pending.edit.source.key, pending.device.hash, pending.device.body);
					} catch {
						// Its length is not the view's count: the server's list is used when it names it.
					}
				}
				return !code;
			});
		}
		if (this.#view !== confirmed) this.#view?.free();
		this.#view = view;
		this.#emit();
	}

	/** Says an edit did not stay. The caller removes it from `#pending`. */
	#drop(pending: Pending, code: string, message: string) {
		this.#refused({ code, message, edit: pending.edit });
	}

	/** Lets go of both queues' memory. */
	#release() {
		if (this.#view !== this.#confirmed) this.#view?.free();
		this.#confirmed?.free();
		this.#confirmed = this.#view = null;
	}

	/** Tells the page what changed. */
	#emit() {
		const view = this.#view;
		const state: {
			current: string | null;
			repeat: Repeat;
			shuffle: unknown;
			sources: { key: string; libraryId: string; list?: string }[];
		} | null = view ? JSON.parse(view.toJson()) : null;
		this.#sources = new Map(state?.sources.map((source) => [source.key, source]));
		this.#shuffled = !!state?.shuffle;
		this.#current = state?.current ?? null;
		// What clean-up dropped, the view's rows for it go too.
		for (const key of this.#viewRows.keys())
			if (!this.#sources.has(key)) this.#viewRows.delete(key);
		const heldRow = this.#starting ? null : this.#confirmed?.currentRow();
		this.#holding = view ? this.#holdingIn(view) : { upNext: [], history: [] };
		const manual = view?.manualLen() ?? 0;
		this.#changed({
			queueId: this.#queueId,
			context: this.#context,
			version: this.#version,
			revision: ++this.#revision,
			current: state?.current ?? null,
			playingManual: view?.playingManual() ?? false,
			held: heldRow ? (JSON.parse(heldRow) as CoreRow).item : null,
			manual: manual - this.#holding.upNext.filter((at) => at < manual).length,
			upNext: (view?.upNextLen() ?? 0) - this.#holding.upNext.length,
			history: (view?.historyLen() ?? 0) - this.#holding.history.length,
			repeat: state?.repeat ?? 'off',
			shuffled: !!state?.shuffle
		});
		this.#sync();
		this.#changedForSave();
	}

	/**
	 * The rows of `view` that hold a hidden track: where each attached list holds one, found
	 * once for each list, then the rows that read those positions. None while nothing is hidden.
	 */
	#holdingIn(view: Queue): Holding {
		const none = { upNext: [], history: [] };
		const hidden = [...this.#hidden.values()].flatMap((tracks) => [...tracks]);
		if (hidden.length === 0) return none;
		try {
			const tracks = JSON.stringify(hidden);
			const hits: Record<string, number[]> = {};
			for (const hash of JSON.parse(view.listHashes()) as string[]) {
				let positions = this.#hits.get(hash);
				if (!positions) {
					const found = view.positionsHolding(hash, tracks);
					if (found === undefined) continue;
					positions = JSON.parse(found) as number[];
					this.#hits.set(hash, positions);
				}
				if (positions.length > 0) hits[hash] = positions;
			}
			return Object.keys(hits).length > 0 ? JSON.parse(view.holding(JSON.stringify(hits))) : none;
		} catch {
			// An ID that is not a track's holds no row.
			return none;
		}
	}

	/**
	 * Settles what the lists and rows need after a change: lists the clean-up dropped are let go
	 * of, the others are loaded in the background, and the rows the page will show first, the
	 * item playing, Up Next's first page and History's, are fetched as pages if no list gives them.
	 */
	#sync() {
		const confirmed = this.#confirmed;
		const view = this.#view;
		if (!confirmed || !view) return;
		const referenced = new Set([...hashesOf(confirmed), ...hashesOf(view)]);
		for (const hash of this.#referenced) {
			if (referenced.has(hash)) continue;
			this.#attached.delete(hash);
			this.#problems.delete(hash);
			this.#hits.delete(hash);
			void this.#lists.delete(hash);
		}
		this.#referenced = referenced;
		for (const { libraryId } of this.#sources.values()) {
			if (libraryId && !this.#hiddenAsked.has(libraryId)) void this.#readHidden(libraryId);
		}
		const rows = (section: 'upNext' | 'history'): CoreRow[] =>
			this.#coreRows(view, section, 0, PAGE);
		if (this.#coreRows(view, 'current', 0, 1).some((row) => this.#needsPage(row))) {
			this.#ensurePages('current', 0, 1);
		}
		for (const section of ['upNext', 'history'] as const) {
			if (rows(section).some((row) => this.#needsPage(row))) this.#ensurePages(section, 0, PAGE);
		}
		void this.#loadLists();
	}

	/** Loads the lists the confirmed queue lacks, one at a time. */
	async #loadLists() {
		if (this.#loading) return;
		this.#loading = true;
		try {
			// Decided with no wait before the flag is cleared, so a change in between is not missed.
			for (let hash = this.#nextList(); hash; hash = this.#nextList()) await this.#load(hash);
		} finally {
			this.#loading = false;
		}
	}

	/**
	 * The next list to load, or null: one that the item playing or Up Next's first rows read,
	 * else any. A list that cannot be had is not asked for again.
	 */
	#nextList(): string | null {
		const confirmed = this.#confirmed;
		const view = this.#view;
		if (!confirmed || !view) return null;
		const wanted = hashesOf(confirmed).filter(
			(hash) => !this.#attached.has(hash) && !this.#problems.has(hash)
		);
		const first = new Set(
			[...this.#coreRows(view, 'current', 0, 1), ...this.#coreRows(view, 'upNext', 0, PAGE)].map(
				(row) => this.#sources.get(row.source)?.list
			)
		);
		return wanted.find((hash) => first.has(hash)) ?? wanted[0] ?? null;
	}

	/** Takes the list `hash` from storage or the server, and attaches it. */
	async #load(hash: string) {
		let body = await this.#lists.get(hash);
		const stored = body !== null;
		if (body === null) {
			const got = await download((path) => this.#get(path), hash);
			if (typeof got === 'string') {
				this.#problems.set(hash, got);
				this.#emit();
				return;
			}
			body = got.body;
		}
		if (!this.#referenced.has(hash)) return;
		if (!this.#attach(hash, body)) {
			// Not a list this queue can use; asking again would only get the same.
			this.#problems.set(hash, 'gone');
			void this.#lists.delete(hash);
			this.#emit();
			return;
		}
		if (!stored) void this.#lists.put(hash, body);
		this.#rebuild();
	}

	/** Attaches a list to the confirmed queue, and drops what pages gave for its sources; false if it does not fit. */
	#attach(hash: string, body: string): boolean {
		try {
			this.#confirmed?.attachList(hash, body);
		} catch {
			return false;
		}
		this.#attached.add(hash);
		const keys = new Set<string>();
		for (const [key, source] of this.#sources) if (source.list === hash) keys.add(key);
		this.#pages.forgetSources(keys);
		for (const key of keys) this.#viewRows.delete(key);
		return true;
	}

	/**
	 * Fetches the queue while it is due: once every start, edit and restore this instance gave the
	 * log is settled, and every one the log held as the app opened has left it, so the server's queue
	 * holds them; and not again after a fetch that failed until the server is heard from. Asked for
	 * while it runs, it looks again once done.
	 */
	async #drain() {
		if (this.#draining) {
			this.#drainAgain = true;
			return;
		}
		this.#draining = true;
		try {
			do {
				this.#drainAgain = false;
				while (
					!this.#wiped &&
					this.#refetch &&
					this.#own.size === 0 &&
					this.#failedAt !== this.#reached
				) {
					// Checked only while there is something to check, so a fetch otherwise leaves at once.
					if (this.#replayed.size > 0 && !(await this.#replayedSent())) break;
					await this.#fetchQueue();
				}
			} while (this.#drainAgain);
		} finally {
			this.#draining = false;
		}
	}

	/**
	 * Whether every start, edit and restore the log held as the app opened has left it, sent or
	 * dropped by whichever tab. A database that cannot be read counts as none left, so it does not hold the fetch back.
	 */
	async #replayedSent(): Promise<boolean> {
		try {
			const log = await this.#port!.all<Mutation>('mutations');
			const left = new Set(log.map((mutation) => idOf(mutation as unknown as QueueMutation)));
			for (const id of this.#replayed) if (!left.has(id)) this.#replayed.delete(id);
		} catch {
			this.#replayed.clear();
		}
		return this.#replayed.size === 0;
	}

	/**
	 * Loads the server's queue, and puts the pending edits back on top. The request names the
	 * queue and version held, so the server answers `304`, with nothing to download, if that is
	 * still its queue.
	 */
	async #fetchQueue() {
		this.#refetch = false;
		const epoch = this.#epoch;
		// Held until the snapshot is installed, which can wait for stored lists.
		const held: QueueEditMessage[] = (this.#held = []);
		let settle = false;
		const reached = this.#reached;
		try {
			const response = await this.#get(
				'/me/queue',
				this.#run !== null && this.#queueId
					? { 'If-None-Match': `"${this.#run}.${this.#queueId}.${this.#version}"` }
					: {}
			);
			// A start meanwhile made this the queue of another, which is not to replace it.
			if (epoch !== this.#epoch) return;
			if (!response || response.status >= 500 || response.status === 429) {
				this.#refetch = true;
				this.#failedAt = reached;
				return;
			}
			this.#run = runOf(response.headers.get('ETag')) ?? this.#run;
			if (response.status === 404) {
				this.#clear();
			} else if (response.status === 304) {
				settle = this.#pending.length > 0 || held.length > 0;
			} else if (response.ok) {
				settle = await this.#install((await response.json()) as QueueSnapshot, epoch);
			}
		} finally {
			this.#held = null;
		}
		if (settle) this.#caughtUp(held);
	}

	/** The account has no queue; what was pending went with it, and so do its lists. */
	#clear() {
		this.#release();
		this.#queueId = null;
		this.#context = null;
		this.#version = 0;
		this.#attached.clear();
		this.#referenced.clear();
		this.#problems.clear();
		this.#pages.reset(null);
		void this.#lists.keep(new Set());
		this.#pending = [];
		this.#starting = null;
		this.#emit();
	}

	/**
	 * Makes the snapshot the confirmed queue, with its lists attached again, since a new core
	 * queue holds none: those the confirmed queue holds are shared from it, and only the others
	 * are read from storage. Pending edits for another queue go. Stored lists the snapshot does
	 * not use are deleted: after a reload nothing else knows they were kept. False if a start
	 * since `epoch` made the snapshot out of date, which then changes nothing.
	 */
	async #install(
		{ queueId, version, context, state }: QueueSnapshot,
		epoch: number
	): Promise<boolean> {
		const next = Queue.fromJson(JSON.stringify(state));
		const hashes = hashesOf(next);
		const attached = new Set<string>(
			this.#confirmed ? JSON.parse(next.adoptLists(this.#confirmed)) : []
		);
		for (const hash of hashes) {
			if (attached.has(hash)) continue;
			const body = await this.#lists.get(hash);
			if (body === null) continue;
			try {
				next.attachList(hash, body);
				attached.add(hash);
			} catch {
				void this.#lists.delete(hash);
			}
		}
		if (epoch !== this.#epoch) return false;
		void this.#lists.keep(new Set(hashes));
		this.#release();
		// A start the server moved: its current is not the row the view clicked.
		if (this.#startItem !== null) {
			if (state.current !== this.#startItem) this.#viewRows.clear();
			this.#startItem = null;
		}
		this.#confirmed = this.#view = next;
		this.#attached = attached;
		this.#queueId = queueId;
		this.#context = context;
		this.#version = version;
		// The server's queue is what it has, and what one shown again from the log changed shows once
		// the server has it. An edit made here for another queue, not answered, will be refused.
		this.#pending = this.#pending.filter((pending) => {
			if (pending.queueId === queueId && !pending.replayed) return true;
			if (pending.queueId !== queueId && !pending.replayed && pending.version === undefined)
				this.#drop(
					pending,
					'superseded',
					'A different queue was started, so this change did not apply.'
				);
			return false;
		});
		if (this.#starting?.replayed || this.#starting?.body.queueId !== queueId) this.#starting = null;
		return true;
	}

	/**
	 * Settles the pending edits once the server's queue is held at `#version`, then replays
	 * the edits heard during the fetch and rebuilds the view. An answered edit at or before
	 * the version is in the queue, and is not replayed. One answered beyond what the server now
	 * has, and not heard, was lost by it, as in a restart: it is reported and not sent again,
	 * which could apply it twice. If a gap in versions is still to be fetched, that waits.
	 */
	#caughtUp(held: QueueEditMessage[]) {
		this.#pending = this.#pending.filter(
			(pending) => pending.version === undefined || pending.version > this.#version
		);
		for (const message of held) this.#heard(message);
		if (!this.#refetch) {
			this.#pending = this.#pending.filter((pending) => {
				if (pending.version === undefined) return true;
				this.#drop(pending, 'lost', 'The server no longer has this change.');
				return false;
			});
		}
		this.#rebuild();
	}
}

/**
 * How a start, edit or restore is named in `#own` and `#replayed`: `start:`, `edit:` or `restore:`
 * and its ID; empty for any other mutation.
 */
function idOf({ name, args }: QueueMutation): string {
	if (name === 'startQueue') return `start:${args.body.queueId}`;
	if (name === 'editQueue') return `edit:${args.editId}`;
	if (name === 'restoreQueue') return `restore:${args.queueId}`;
	return '';
}

/** Applies `edit` to `queue`; the core's refusal code if it refuses, `invalid` for a malformed edit. */
function applyTo(queue: Queue, edit: QueueEdit): string | undefined {
	try {
		return queue.apply(JSON.stringify(edit));
	} catch {
		return 'invalid';
	}
}

/**
 * The edit as the core applies it here, or null if only the server can resolve it: an `add` of
 * no tracks, or of a `recipe` with no `length`. An add has just its count here, a recipe's
 * `length` or the number of `tracks`; the list attaches later, under the hash the server gives it.
 */
function localEdit(edit: SentEdit, length?: number): QueueEdit | null {
	if (edit.type !== 'add') return edit;
	const { key, libraryId, tracks, recipe } = edit.source;
	const count = recipe ? length : tracks?.length;
	if (!count) return null;
	return { ...edit, source: { key, libraryId, length: count } };
}

/** The run part of an `ETag`, `"<run>.<queueId>.<version>"`, or `null` without one. */
function runOf(etag: string | null): string | null {
	return etag?.replace(/^"/, '').split('.')[0] || null;
}

/** The hashes of the lists `queue` references. */
function hashesOf(queue: Queue): string[] {
	return JSON.parse(queue.listHashes());
}

/**
 * The JSON of the list of this device's own add, as the list's download would give it: its
 * `tracks`, or the recipe's list it resolved if the server named `hash` for it; null if it has
 * none.
 */
function ownBody({ edit, device }: Pending, hash?: string): string | null {
	if (edit.type !== 'add') return null;
	if (edit.source.tracks?.length) {
		return JSON.stringify({ libraryId: edit.source.libraryId, tracks: edit.source.tracks });
	}
	// A recipe's list this device resolved is the server's only if the hashes are the same.
	return device && device.hash === hash ? device.body : null;
}

/**
 * Where row `offset` of what the page is shown is in the whole of its part, past the rows
 * `skip`, ascending, which it is not shown.
 */
function unskipped(offset: number, skip: readonly number[]): number {
	let at = offset;
	for (const row of skip) {
		if (row > at) break;
		at++;
	}
	return at;
}
