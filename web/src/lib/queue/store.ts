// The account's queue on this device: the server's queue as confirmed, plus this device's own
// edits that the server has not yet confirmed, replayed on top (`design/queue.md` §9,
// `requirements/general.md` §3.4). The order is the shared core's, run in WebAssembly, so this
// holds no rule about it, and it holds no tracks in its state. Tracks come from the lists, which
// it downloads by hash in the background and attaches to the core, and meanwhile from pages of
// rows (`design/queue.md` §3). Plain TypeScript so it runs in a worker and in tests; `worker.ts`
// is its shell.

import type { CatalogRouter } from '$lib/catalog/router';
import { Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type {
	PlayContext,
	QueueEdit,
	QueueRecipe,
	QueueSnapshot,
	RealtimeServerMessage,
	RestoredQueue,
	SentEdit,
	StartQueue
} from '$lib/api/types';
import { download, ListCache } from './lists';
import { PAGE, RowPages } from './rows';
import type {
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
	fetch?: typeof fetch;
	/** How long to wait before sending again what could not be sent. */
	retryMs?: number;
	/** Where lists are kept; the browser's Cache Storage if left out. Without any, memory. */
	storage?: CacheStorage;
	/** The way to the device's catalog, whichever tab owns it, for the queue to resolve lists with. */
	catalog?: Pick<CatalogRouter, 'ask'>;
	/** Called after every change, with what the page shows. */
	changed: (summary: Summary) => void;
	/** Called when an edit could not be kept. */
	refused: (refusal: Refusal) => void;
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
	/** The list this device resolved for an `add` of a recipe, once it has: JSON as a list's download gives it. */
	device?: { hash: string; body: string };
};

/** A restore waiting to be sent, and how its answer reaches the caller. */
type Restoring = { queueId: string; settle: (result: RestoreResult) => void };

/** A start this device made that the server has not answered. */
type Starting = {
	/** What is sent: the request without what only this device holds. */
	body: StartQueue;
	/** How long the view said the source was, to tell whether the server's list is the view's. */
	total?: number;
};

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

/** Why a list cannot be had: the server has no such list, or it could not be reached. */
type Problem = 'gone' | 'failed';

type QueueEditMessage = Extract<RealtimeServerMessage, { type: 'queueEdit' }>;

/** The server's problem body, if it sent one. */
type ProblemBody = { code?: string; detail?: string; title?: string } | null;

export class QueueStore {
	#api: string;
	#fetch: typeof fetch;
	#retryMs: number;
	#changed: (summary: Summary) => void;
	#refused: (refusal: Refusal) => void;

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
	/** The start waiting to be sent or answered; it goes before every edit and fetch. */
	#starting: Starting | null = null;
	/** The restore waiting its turn or being sent. */
	#restoring: Restoring | null = null;
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

	/** Makes an empty store; it loads the server's queue when the connection says hello. */
	constructor({
		api,
		fetch: fetcher = (...args) => fetch(...args),
		retryMs = 2000,
		storage = globalThis.caches,
		catalog,
		changed,
		refused
	}: Options) {
		this.catalog = catalog;
		this.#api = api;
		this.#fetch = fetcher;
		this.#retryMs = retryMs;
		this.#changed = changed;
		this.#refused = refused;
		this.#lists = new ListCache(storage, api);
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
		const raw = this.#coreRows(view, section, offset, limit);
		const total =
			section === 'upNext'
				? view.upNextLen()
				: section === 'history'
					? view.historyLen()
					: raw.length;
		if (raw.some((row) => this.#needsPage(row))) this.#ensurePages(section, offset, limit);
		// The page that holds the window's rows: the item playing comes with Up Next's first.
		const [part, first] = section === 'current' ? (['upNext', 0] as const) : [section, offset];
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
						  (problem === 'failed' && !this.#pages.fetching(part, first + i))
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

	/** One request to the API, with no wait after a failure; null when it could not be made. */
	async #get(path: string): Promise<Response | null> {
		return this.#fetch(`${this.#api}${path}`, { headers: { 'X-Jewelcase-Client': 'web' } }).catch(
			() => null
		);
	}

	/**
	 * Starts a queue here at once, as a start from a view does (`design/queue.md` §3): the
	 * queue is the new one, its source as long as the view says and with no list, the clicked row
	 * playing, and the request goes to the server in the background, before anything else. The view's
	 * own rows fill the unshuffled rows they cover. Answers false, changing nothing, if the
	 * start is not a position of the source. The new queue keeps the confirmed queue's repeat
	 * mode, as the server's does (`requirements/queue.md` §5); the latest repeat change not yet
	 * confirmed is then made again on it, and sent after the start, so both end on that mode.
	 * A start of a recipe is also resolved on the device (see `#attachResolved`); `offline` says
	 * whether the app counts itself offline, and is not sent to the server.
	 */
	start({ total, loaded, offline, ...body }: StartRequest): boolean {
		const { key, libraryId, tracks } = body.source;
		const length = tracks?.length ?? total ?? 0;
		// The core makes the state; it rounds what it is given, so a position it would not is refused here.
		if (!Number.isInteger(body.start) || body.start < 0) return false;
		let next: Queue;
		try {
			const repeat = this.#confirmed ? JSON.parse(this.#confirmed.toJson()).repeat : 'off';
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
		const tapped = this.#pending.findLast((pending) => pending.edit.type === 'repeat');
		this.#supersede((pending) => pending.edit.type !== 'repeat');
		this.#pending = [];
		this.#starting = { body, total };
		if (body.source.recipe)
			void this.#attachResolved(key, libraryId, body.source.recipe, !!offline);
		if (tapped) {
			this.edit(tapped.edit);
		} else {
			this.#emit();
			this.#drain();
		}
		return true;
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
			const request = { type: 'query', library: libraryId, name: 'resolveRecipe', params } as const;
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
	 * Sends the start, again if the answer was lost. Its answer gives the list's hash: a start of
	 * `tracks` attaches them under it, so they are never downloaded back, and a recipe's start
	 * fetches the queue, since the server's list and start may differ from the view's. A start
	 * the server refuses is gone, and the queue the server has is fetched.
	 */
	async #sendStart(starting: Starting) {
		const { body } = starting;
		const response = await this.#call('POST', '/me/queue', body);
		if (this.#starting !== starting || !response) return;
		if (!response.ok) {
			const problem = await problemOf(response);
			this.#starting = null;
			this.#viewRows.clear();
			this.#supersede((pending) => pending.queueId === body.queueId);
			this.#refetch = true;
			this.#refused({ code: problem.code, message: problem.message, edit: null });
			return;
		}
		const { list, length } = (await response.json()) as { list: string; length: number };
		this.#starting = null;
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
	 * Makes the kept queue `queueId` the account's active one (`POST /me/queues/{id}/restore`).
	 * It waits its turn after a start and the edits pending, and goes before a fetch, so an
	 * unanswered start cannot replace it and no fetch runs beside it. Nothing shows until the
	 * server answers, since only it holds the queue; the answer is then installed as a fetch's is,
	 * and edits for the queue it replaced are dropped. The request is not retried, since a second
	 * restore of the same queue is refused as active. Settles with where playback was left, or why
	 * the queue was not restored; one restore at a time.
	 */
	restore(queueId: string): Promise<RestoreResult> {
		if (this.#restoring) {
			return Promise.resolve({ ok: false, code: 'busy', message: 'Another restore is under way.' });
		}
		return new Promise((settle) => {
			this.#restoring = { queueId, settle };
			void this.#drain();
		});
	}

	/** Sends the restore and installs its answer; see [`restore`](#restore). A restore asked for meanwhile is `busy` until it is answered. */
	async #sendRestore({ queueId, settle }: Restoring) {
		try {
			const epoch = ++this.#epoch;
			const path = `/me/queues/${encodeURIComponent(queueId)}/restore`;
			const response = await this.#call('POST', path);
			if (!response?.ok) {
				if (!response) {
					// Whether it was applied is not known: the queue is fetched to find out.
					this.#refetch = true;
					return settle({
						ok: false,
						code: 'unreachable',
						message: 'The server could not be reached.'
					});
				}
				const { code, message } = await problemOf(response);
				return settle({ ok: false, code, message });
			}
			const restored = (await response.json()) as RestoredQueue;
			this.#run = runOf(response.headers.get('ETag')) ?? this.#run;
			this.#startItem = null;
			this.#viewRows.clear();
			if (!(await this.#install(restored, epoch))) {
				return settle({
					ok: false,
					code: 'superseded',
					message: 'Another queue replaced this one.'
				});
			}
			// An edit heard before this was installed named another queue, so a fetch follows for it.
			this.#caughtUp([]);
			settle({ ok: true, position: restored.position ?? null });
		} finally {
			this.#restoring = null;
		}
	}

	/**
	 * Applies `edit` here at once and sends it in the background, once, however many times the
	 * send is tried: it carries a fresh `editId`. An edit the core refuses changes nothing and is
	 * reported. An `add` of a `recipe` applies here as `length` rows, the count of the view it
	 * comes from, which wait for the server's list or pages (`design/queue.md` §3); the server's
	 * `queueEdit` gives the length it resolved. Without a length it is just sent, and shows when
	 * that arrives. An `add` of a recipe is also resolved on the device, and its rows show from
	 * that list at once (see `#resolveAdd`); `offline` says whether the app counts itself offline,
	 * which lets a search be resolved there too.
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
		const pending: Pending = { editId: crypto.randomUUID(), queueId: this.#queueId, edit, local };
		this.#pending.push(pending);
		if (edit.type === 'add' && edit.source.recipe) {
			void this.#resolveAdd(pending, edit.source.libraryId, edit.source.recipe, offline);
		}
		this.#emit();
		this.#drain();
	}

	/** Takes in what the server said over the realtime connection. */
	message(message: RealtimeServerMessage) {
		if (message.type === 'hello') {
			// What could not be reached is asked for again.
			for (const [hash, problem] of this.#problems) {
				if (problem === 'failed') this.#problems.delete(hash);
			}
			this.#pages.retry();
			this.#refetch = true;
			this.#drain();
			this.#sync();
		} else if (message.type === 'queueReplaced') {
			// A fetch under way may answer with the queue this replaces, so it is fetched again.
			if (this.#held || message.queueId !== this.#queueId) {
				this.#refetch = true;
				this.#drain();
			}
		} else if (message.type === 'queueEdit') {
			this.#heard(message);
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
		this.#changed({
			queueId: this.#queueId,
			context: this.#context,
			version: this.#version,
			revision: ++this.#revision,
			current: state?.current ?? null,
			playingManual: view?.playingManual() ?? false,
			held: heldRow ? (JSON.parse(heldRow) as CoreRow).item : null,
			manual: view?.manualLen() ?? 0,
			upNext: view?.upNextLen() ?? 0,
			history: view?.historyLen() ?? 0,
			repeat: state?.repeat ?? 'off',
			shuffled: !!state?.shuffle
		});
		this.#sync();
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
			void this.#lists.delete(hash);
		}
		this.#referenced = referenced;
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

	/** Sends and fetches what is waiting, one request at a time, so the server hears edits in order. */
	async #drain() {
		if (this.#draining) return;
		this.#draining = true;
		try {
			// Decided and cleared with no wait between, so what arrives meanwhile is never missed.
			for (let work = this.#next(); work; work = this.#next()) await work();
		} finally {
			this.#draining = false;
		}
	}

	/**
	 * The next thing waiting, or null: the start, an edit, a restore, then a fetch. Edits go before the fetch so each is
	 * answered, or known unsent, by the time the snapshot is installed.
	 */
	#next(): (() => Promise<void>) | null {
		const starting = this.#starting;
		if (starting) return () => this.#sendStart(starting);
		const edit = this.#pending.find((pending) => pending.version === undefined);
		if (edit) return () => this.#sendEdit(edit);
		const restoring = this.#restoring;
		if (restoring) return () => this.#sendRestore(restoring);
		return this.#refetch ? () => this.#fetchQueue() : null;
	}

	/** One request to the API; null when it could not be made, or the server was too busy or broken. */
	async #call(
		method: 'GET' | 'POST',
		path: string,
		body?: object,
		extra: Record<string, string> = {}
	): Promise<Response | null> {
		const headers: Record<string, string> = { 'X-Jewelcase-Client': 'web', ...extra };
		if (body) headers['Content-Type'] = 'application/json';
		const response = await this.#fetch(`${this.#api}${path}`, {
			method,
			headers,
			body: body && JSON.stringify(body)
		}).catch(() => null);
		if (response && response.status < 500 && response.status !== 429) {
			return response;
		}
		await new Promise((resolve) => setTimeout(resolve, this.#retryMs));
		return null;
	}

	/** Sends one edit, again with the same `editId` if the answer was lost. */
	async #sendEdit(pending: Pending) {
		const response = await this.#call('POST', '/me/queue/edits', {
			editId: pending.editId,
			queueId: pending.queueId,
			edit: pending.edit
		});
		if (!response || !this.#pending.includes(pending)) return;
		if (response.ok) {
			const applied = (await response.json()) as { version: number; source?: { list?: string } };
			pending.version = applied.version;
			// Kept under the hash it names, so a snapshot that has the add never downloads it back.
			const body = ownBody(pending, applied.source?.list);
			if (applied.source?.list && body) void this.#lists.put(applied.source.list, body);
			return;
		}
		const problem = await problemOf(response);
		this.#pending = this.#pending.filter((other) => other !== pending);
		const replaced = problem.code === 'queue_changed';
		if (replaced || response.status === 404) this.#refetch = true;
		this.#drop(pending, replaced ? 'superseded' : problem.code, problem.message);
		this.#rebuild();
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
		try {
			const response = await this.#call(
				'GET',
				'/me/queue',
				undefined,
				this.#run !== null && this.#queueId
					? { 'If-None-Match': `"${this.#run}.${this.#queueId}.${this.#version}"` }
					: {}
			);
			// A start meanwhile made this the queue of another, which is not to replace it.
			if (epoch !== this.#epoch) return;
			if (!response) {
				this.#refetch = true;
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
		this.#supersede(() => true);
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
		this.#supersede((pending) => pending.queueId !== queueId);
		return true;
	}

	/** Drops the pending edits `gone` picks, saying a new queue replaced them. */
	#supersede(gone: (pending: Pending) => boolean) {
		this.#pending = this.#pending.filter((pending) => {
			if (!gone(pending)) return true;
			this.#drop(pending, 'superseded', 'A new queue replaced this change.');
			return false;
		});
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

/** The reason the server refused a request. */
async function problemOf(response: Response): Promise<{ code: string; message: string }> {
	const problem: ProblemBody = await response.json().catch(() => null);
	return {
		code: problem?.code ?? 'failed',
		message: problem?.detail ?? problem?.title ?? response.statusText
	};
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
 * `tracks`, or the recipe's list it resolved if the server named `hash` for it; null if it has none.
 */
function ownBody({ edit, device }: Pending, hash?: string): string | null {
	if (edit.type !== 'add') return null;
	if (edit.source.tracks?.length) {
		return JSON.stringify({ libraryId: edit.source.libraryId, tracks: edit.source.tracks });
	}
	// A recipe's list this device resolved is the server's only if the hashes are the same.
	return device && device.hash === hash ? device.body : null;
}
