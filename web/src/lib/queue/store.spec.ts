import { readFileSync } from 'node:fs';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { initSync, Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type {
	KeptQueue,
	QueueEdit,
	RealtimeServerMessage,
	SentEdit,
	StartQueue
} from '$lib/api/types';
import { connectivity } from '$lib/connectivity.svelte';
import type { Device } from '$lib/sync/device';
import { Sender } from '$lib/sync/sender';
import { type Env, type StoreName, type Stored, memoryEnv, unsent } from '$lib/sync/store';
import type { QueueMutation, Refusal, Summary } from './protocol';
import { KEPT_MS, type KeptRecord, type QueueRecord, QueueStore } from './store';

vi.mock('$app/paths', () => ({ base: '', resolve: (path: string) => path }));

/** Loads the core's WebAssembly, which the browser would otherwise fetch. */
beforeAll(() => {
	initSync({
		module: readFileSync(new URL('../core-wasm/jewelcase_core_wasm_bg.wasm', import.meta.url))
	});
});

const API = '/api/v1';
const QUEUE = '11111111-1111-4111-8111-111111111111';
const CONTEXT = { type: 'album', id: '1' } as const;

/** The IDs of `key`'s items at these positions. */
const ids = (key: string, ...positions: number[]) => positions.map((i) => `${key}.${i}`);

/** A queue playing source `a` of five tracks from its first, as the server's state has it. */
function albumQueue(): Queue {
	return Queue.fromJson(
		JSON.stringify({
			sources: [{ key: 'a', libraryId: 'lib', list: 'h', length: 5 }],
			copies: [],
			manual: [],
			order: [{ view: { key: 'a' }, start: 0, len: 5 }],
			cursor: 1,
			current: 'a.0',
			history: [],
			nextPlay: 0,
			shuffle: null,
			repeat: 'off',
			adjusted: false
		})
	);
}

type Request = { method: string; path: string; body?: any; headers?: Record<string, string> }; // eslint-disable-line @typescript-eslint/no-explicit-any

/** A server holding one queue in the core, answering as the API does. */
class FakeServer {
	queue = albumQueue();
	queueId = QUEUE;
	version = 0;
	/** New each time the server starts, and part of every `ETag`. */
	run = 'r1';
	requests: Request[] = [];
	/** Statuses to answer the next requests with instead, in order; 0 is a lost connection. */
	failures: number[] = [];
	/** Runs when a fetch of the queue has been answered but before the store hears the answer. */
	onGet: (() => void) | null = null;
	/** The lists the server holds, by hash. */
	lists = new Map<string, string[]>([['h', ['100', '101', '102', '103', '104']]]);
	/** Holds back the answers to requests for lists or for rows until it is opened. */
	listGate: Promise<void> | null = null;
	/** Holds back the answers to requests for one list, by hash, until each is opened. */
	listHolds = new Map<string, Promise<void>>();
	rowsGate: Promise<void> | null = null;
	/** Holds back the answer to an edit, once it is applied, until it is opened. */
	editGate: Promise<void> | null = null;
	/** Runs once a restore is answered: what the server does next. */
	afterRestore: (() => void) | null = null;
	/** Holds back the answer to a restore until it is opened. */
	restoreGate: Promise<void> | null = null;
	/** Holds back the answer to a start until it is opened.*/
	startGate: Promise<void> | null = null;
	/** What a recipe resolves to, as the tracks of the list the server holds for a recipe's start. */
	resolved5 = ['200', '201', '202', '203', '204'];
	/** The queues a start replaced that were adjusted, which the server keeps, in order. */
	parked: string[] = [];
	/** The kept queues, by ID: the queue, where playback was left, and when it was parked. */
	kept = new Map<
		string,
		{ queue: Queue; position: { item: string; positionMs: number } | null; at?: string }
	>();
	/** Holds back the answers to reads of kept queues until it is opened. */
	keptGate: Promise<void> | null = null;
	/** Kept queues cannot be read while set: their reads fail as with no connection. */
	keptDown = false;
	/** A day ago: when a kept queue with no time of its own was parked, a second apart. */
	keptSince = Date.now() - 86_400_000;
	/** The status pages of rows are answered with instead of the page. */
	rowsStatus = 0;
	/** Edits applied whose answer is lost on its way, in count. */
	loseAnswers = 0;
	/** The tracks the user hid, by library, as Hidden Songs lists them, `hiddenPage` at a time. */
	hidden = new Map<string, string[]>();
	hiddenPage = 1000;
	/** Holds back the answers to reads of Hidden Songs until it is opened. */
	hiddenGate: Promise<void> | null = null;
	#answered = new Map<string, number>();

	/** An edit another device made: applied here, and the message every device hears. */
	apply(edit: QueueEdit, editId = crypto.randomUUID()): RealtimeServerMessage {
		const code = this.queue.apply(JSON.stringify(edit));
		if (code) throw new Error(code);
		this.version++;
		this.#answered.set(editId, this.version);
		return { type: 'queueEdit', queueId: this.queueId, version: this.version, editId, edit };
	}

	/** An edit as the server registers it: an add's source carries its list and length. */
	resolved(edit: SentEdit): QueueEdit {
		if (edit.type !== 'add') return edit;
		const { key, libraryId, tracks } = edit.source;
		const length = tracks ? tracks.length : 2;
		const list = tracks ? `h-${tracks.join('-')}` : `h-${key}`;
		return { ...edit, source: { key, libraryId, list, length } };
	}

	/** The `queueEdit` the server sent for the `n`th edit this device sent (the last by default). */
	heardOf(n = -1): Extract<RealtimeServerMessage, { type: 'queueEdit' }> {
		const sent = this.requests.filter((r) => r.path === '/me/queue/edits');
		const { editId, edit } = sent.at(n)!.body;
		return {
			type: 'queueEdit',
			queueId: this.queueId,
			version: this.#answered.get(editId)!,
			editId,
			edit: this.resolved(edit)
		};
	}

	/** Starts a queue as `POST /me/queue` does, over the tracks sent or the recipe's resolve. */
	start({ queueId, source, start, startTrackId }: StartQueue): Response {
		const tracks: string[] = source.tracks ?? this.resolved5;
		const at = source.tracks ? start : tracks.indexOf(startTrackId!);
		const list = `hs-${queueId.slice(0, 4)}`;
		const { repeat, adjusted } = JSON.parse(this.queue.toJson());
		if (adjusted) this.park();
		const state = JSON.parse(Queue.started(source.key, 'lib', tracks.length, at, repeat).toJson());
		state.sources[0].list = list;
		this.queue = Queue.fromJson(JSON.stringify(state));
		this.queueId = queueId;
		this.version = 0;
		this.lists.set(list, tracks);
		return new Response(JSON.stringify({ list, length: tracks.length }));
	}

	/** Keeps the active queue, which a start or restore replaces after it was adjusted. */
	park() {
		this.parked.push(this.queueId);
		const at = new Date(Date.now() + this.parked.length).toISOString();
		this.kept.set(this.queueId, { queue: this.queue, position: null, at });
	}

	/**
	 * The kept queues as `listKeptQueues` gives them, most recently parked first, with a tag over
	 * them; `304` when `known` names it.
	 */
	keptList(known?: string) {
		const items = [...this.kept].map(([queueId, { queue, at }], i) => ({
			queueId,
			context: CONTEXT,
			lastActive: at ?? new Date(this.keptSince + i * 1000).toISOString(),
			current: { libraryId: 'lib', trackId: '100' },
			upNext: queue.upNextLen(),
			history: queue.historyLen()
		}));
		items.sort((a, b) => b.lastActive.localeCompare(a.lastActive));
		const tag = `"k${items.map((item) => `${item.queueId}@${item.lastActive}`).join(',')}"`;
		if (known === tag) return new Response(null, { status: 304, headers: { ETag: tag } });
		const body = JSON.stringify({ items, nextCursor: null, total: items.length });
		return new Response(body, { headers: { ETag: tag } });
	}

	/** Starts the server again: a new run, and the queue as a crash left it. */
	restart(queue: Queue, version: number) {
		this.run = 'r2';
		this.queue = queue;
		this.version = version;
		this.#answered.clear();
	}

	/** The requests for the queue itself. */
	queueGets() {
		return this.requests.filter((r) => r.path === '/me/queue' && r.method === 'GET');
	}

	/** How many times the queue was asked for. */
	gets() {
		return this.queueGets().length;
	}

	/** The requests for lists, as the hashes asked for, in order. */
	listGets() {
		return this.requests
			.filter((r) => r.path.startsWith('/me/queue/lists/'))
			.map((r) => r.path.split('/').pop());
	}

	/** The requests for pages of rows, as `section:offset`. */
	rowGets() {
		return this.requests
			.filter((r) => r.path.startsWith('/me/queue/rows'))
			.map((r) => {
				const query = new URL(r.path, 'http://x').searchParams;
				return `${query.get('section')}:${query.get('offset')}`;
			});
	}

	/** The page of rows the server's queue, with every list it holds, gives. */
	rowsPage(path: string): Response {
		const query = new URL(path, 'http://x').searchParams;
		const [offset, limit] = [Number(query.get('offset')), Number(query.get('limit'))];
		const queue = this.queue.copy();
		for (const [hash, tracks] of this.lists) {
			queue.attachList(hash, JSON.stringify({ libraryId: 'lib', tracks }));
		}
		const history = query.get('section') === 'history';
		/** A core row as the API's page gives it. */
		const row = ({ item, track, play }: { item: string; track: string | null; play?: number }) => ({
			item,
			libraryId: 'lib',
			trackId: track,
			...(play === undefined ? {} : { play })
		});
		const rows = JSON.parse(
			history ? queue.historyRows(offset, limit) : queue.upNextRows(offset, limit)
		);
		const current = queue.currentRow();
		const page = {
			queueId: this.queueId,
			version: this.version,
			current: current && row(JSON.parse(current)),
			total: history ? queue.historyLen() : queue.upNextLen(),
			manual: history ? 0 : queue.manualLen(),
			shuffled: false,
			repeat: 'off',
			rows: rows.map(row)
		};
		queue.free();
		return new Response(JSON.stringify(page));
	}

	/** Answers a request as the API does, unless a failure is queued for it. */
	fetch = async (url: string | URL | Request, init?: RequestInit): Promise<Response> => {
		const path = String(url).slice(API.length);
		const method = init?.method ?? 'GET';
		const body = typeof init?.body === 'string' ? JSON.parse(init.body) : undefined;
		const headers = init?.headers as Record<string, string> | undefined;
		this.requests.push({ method, path, body, headers });
		if (path.startsWith('/me/queue/lists/')) {
			await this.listGate;
			await this.listHolds.get(path.split('/').pop()!);
			const tracks = this.lists.get(path.split('/').pop()!);
			if (!tracks) return problem(404, 'not_found');
			return new Response(JSON.stringify({ libraryId: 'lib', tracks }));
		}
		if (path.startsWith('/me/queue/rows')) {
			await this.rowsGate;
			return this.rowsStatus ? problem(this.rowsStatus, 'failed') : this.rowsPage(path);
		}
		const hiddenRead = /^\/libraries\/([^/]+)\/hidden-tracks\?(.*)$/.exec(path);
		if (hiddenRead) {
			await this.hiddenGate;
			const at = Number(new URLSearchParams(hiddenRead[2]).get('cursor') ?? 0);
			const all = this.hidden.get(decodeURIComponent(hiddenRead[1])) ?? [];
			const end = at + this.hiddenPage;
			const items = all.slice(at, end).map((id) => ({ id }));
			const nextCursor = end < all.length ? String(end) : null;
			return new Response(JSON.stringify({ items, nextCursor, total: all.length }));
		}
		if (path.startsWith('/me/queues') && method === 'GET') {
			await this.keptGate;
			if (this.keptDown) throw new TypeError('offline');
			if (path.startsWith('/me/queues?')) return this.keptList(headers?.['If-None-Match']);
			const kept = this.kept.get(decodeURIComponent(path.split('/').pop()!));
			if (!kept) return problem(404, 'not_found');
			const state = JSON.parse(kept.queue.toJson());
			const snapshot = { queueId: path.split('/').pop(), version: 7, context: CONTEXT, state };
			return new Response(JSON.stringify({ ...snapshot, position: kept.position ?? undefined }));
		}
		const deleting = /^\/me\/queues\/([^/]+)$/.exec(path)?.[1];
		if (deleting && method === 'DELETE') {
			if (deleting === this.queueId) return problem(409, 'queue_active');
			return this.kept.delete(deleting)
				? new Response(null, { status: 204 })
				: problem(404, 'not_found');
		}
		const failure = this.failures.shift();
		if (failure === 0) throw new TypeError('offline');
		if (failure) return problem(failure, 'failed');
		if (path === '/me/queue' && method === 'GET') {
			const tag = `"${this.run}.${this.queueId}.${this.version}"`;
			let answer: Response;
			if (headers?.['If-None-Match'] === tag) {
				answer = new Response(null, { status: 304, headers: { ETag: tag } });
			} else {
				const state = JSON.parse(this.queue.toJson());
				const snapshot = { queueId: this.queueId, version: this.version, context: CONTEXT, state };
				answer = new Response(JSON.stringify(snapshot), { headers: { ETag: tag } });
			}
			this.onGet?.();
			return answer;
		}
		const restoring = /^\/me\/queues\/(.+)\/restore$/.exec(path)?.[1];
		if (restoring && method === 'POST') {
			await this.restoreGate;
			if (restoring === this.queueId) return problem(409, 'queue_active');
			const kept = this.kept.get(restoring);
			if (!kept) return problem(404, 'not_found');
			this.kept.delete(restoring);
			if (JSON.parse(this.queue.toJson()).adjusted) this.park();
			this.queue = kept.queue;
			this.queueId = restoring;
			this.version = 7;
			const state = JSON.parse(this.queue.toJson());
			const snapshot = { queueId: this.queueId, version: 7, context: CONTEXT, state };
			const answer = new Response(JSON.stringify({ ...snapshot, position: kept.position }), {
				headers: { ETag: `"${this.run}.${this.queueId}.7"` }
			});
			this.afterRestore?.();
			return answer;
		}
		if (path === '/me/queue' && method === 'POST') {
			await this.startGate;
			return this.start(body);
		}
		if (body.queueId !== this.queueId) return problem(409, 'queue_changed');
		let version = this.#answered.get(body.editId);
		if (version === undefined) {
			const edit = this.resolved(body.edit);
			try {
				this.apply(edit, body.editId);
			} catch (error) {
				return problem(409, (error as Error).message);
			}
			version = this.version;
			if (this.loseAnswers > 0) {
				this.loseAnswers--;
				throw new TypeError('connection lost');
			}
		}
		await this.editGate;
		const edit = this.resolved(body.edit);
		return new Response(
			JSON.stringify({ version, ...(edit.type === 'add' ? { source: edit.source } : {}) })
		);
	};
}

/** Cache Storage, in memory, as far as lists use it; `broken` makes every call fail. */
class FakeCaches {
	files = new Map<string, string>();
	broken = false;
	/** Holds back every read until it is opened. */
	matchGate: Promise<void> | null = null;
	/** The hashes read, in order. */
	reads: string[] = [];

	/** The one cache. */
	async open(): Promise<Cache> {
		if (this.broken) throw new Error('private mode');
		const files = this.files;
		return {
			match: async (url: string) => {
				this.reads.push(url.split('/').pop()!);
				await this.matchGate;
				return files.has(url) ? new Response(files.get(url)) : undefined;
			},
			put: async (url: string, response: Response) => void files.set(url, await response.text()),
			delete: async (url: string | { url: string }) =>
				files.delete(typeof url === 'string' ? url : new URL(url.url).pathname),
			keys: async () => [...files.keys()].map((url) => ({ url: `http://localhost${url}` }))
		} as unknown as Cache;
	}

	/** The hashes kept. */
	hashes() {
		return [...this.files.keys()].map((url) => url.split('/').pop());
	}

	/** Keeps a list as an earlier run of the worker would have. */
	keep(hash: string, tracks: string[]) {
		this.files.set(`${API}/me/queue/lists/${hash}`, JSON.stringify({ libraryId: 'lib', tracks }));
	}
}

/** The device's catalog as far as the queue asks it: what `resolveRecipe` answers. */
class FakeCatalog {
	/**
	 * The list and hash a recipe resolves to; null as for a playlist that is not the user's, and a
	 * rejection by default, as with no owner.
	 */
	answer: { ids: string[]; hash: string } | null | 'fails' = 'fails';
	/** Holds back the answer until it is opened. */
	gate: Promise<void> | null = null;
	asked: { library: string; name: string; offline?: boolean; user?: string }[] = [];

	/** Answers a query as the catalog's owner does, with JSON; rejects as it does when it cannot. */
	ask = async (request: {
		type: string;
		library?: string;
		name?: string;
		offline?: boolean;
		user?: string;
	}): Promise<string> => {
		this.asked.push({
			library: request.library!,
			name: request.name!,
			offline: request.offline,
			user: request.user
		});
		await this.gate;
		if (this.answer === 'fails') throw new Error('the library is not on this device');
		return JSON.stringify(this.answer && { ...this.answer, recipe: { type: 'album' } });
	};
}

/** A gate to hold answers behind, and the way to open it. */
function gate(): { wait: Promise<void>; open: () => void } {
	let open!: () => void;
	const wait = new Promise<void>((resolve) => (open = resolve));
	return { wait, open };
}

/** An error response with the server's problem body. */
const problem = (status: number, code: string) =>
	new Response(JSON.stringify({ code, detail: code }), { status });

/** Lets the store's requests and retries finish. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 20));

/** The device's catalog, which queue mutations do not touch. */
const noDevice: Device = {
	apply: async () => {},
	rebase: async () => {},
	confirmed: async () => {}
};

/** The kinds of mutation the queue gives the log. */
const QUEUE_MUTATIONS = new Set(['startQueue', 'editQueue', 'restoreQueue', 'deleteKeptQueue']);

/** Every sender a spec made, stopped after it. */
let senders: Sender[] = [];
afterEach(() => {
	for (const one of senders) one.stop();
	senders = [];
	connectivity.setWorkOffline(false);
	connectivity.reach();
});

/** Waits `ms` of real time. */
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * A page's log for user `user` over `env`, sending with `fetcher`, as the page's sender does:
 * `log` is what the store gives its mutations to, and what becomes of each goes to the store
 * `to` gives, as the page tells its worker.
 */
function pageLog(
	fetcher: typeof fetch,
	to: () => QueueStore | undefined,
	{ env = memoryEnv(), user = '7' }: { env?: Env; user?: string } = {}
) {
	const sender = new Sender();
	senders.push(sender);
	sender.start(user, { env, fetch: fetcher, device: noDevice });
	sender.onSettled(({ name, args }, outcome, result) => {
		if (QUEUE_MUTATIONS.has(name)) to()?.settled({ name, args } as QueueMutation, outcome, result);
	});
	const log = {
		enqueue: ({ name, args }: QueueMutation) => void sender.enqueue(name, args as never)
	};
	return { sender, log };
}

let server: FakeServer;
let store: QueueStore;
/** The page's sender the store's mutations go through. */
let sender: Sender;
let summary: Summary;
let refusals: Refusal[];
let caches: FakeCaches;
let device: FakeCatalog;

/** The first `limit` rows of Up Next as the store gives them: each track, or its status if it has none. */
const upNext = (limit = 10) =>
	store.rows({ section: 'upNext', offset: 0, limit }).rows.map((row) => row.track ?? row.status);

/** Says hello, as a connection does, and lets the store catch up. */
async function hello() {
	store.message({ type: 'hello', deviceId: '1', serverTime: new Date().toISOString() });
	await settle();
}

/** The edit that moves to the next item. */
const next: QueueEdit = { type: 'next' };
/** An edit that adds a one-track source `key` to the Manual Queue. */
const queueManual = (key: string): SentEdit => ({
	type: 'add',
	source: { key, libraryId: 'lib', tracks: [`${key}-0`] },
	before: { type: 'manualEnd' }
});

/** Gives each test a fresh server and a store talking to it. */
beforeEach(() => {
	server = new FakeServer();
	refusals = [];
	caches = new FakeCaches();
	device = new FakeCatalog();
	// A store from an earlier test can still be retrying; only this test's store reports here.
	const page = pageLog(server.fetch as typeof fetch, () => store);
	sender = page.sender;
	const created: QueueStore = new QueueStore({
		api: API,
		user: '7',
		fetch: server.fetch as typeof fetch,
		log: page.log,
		storage: caches as unknown as CacheStorage,
		catalog: device,
		changed: (s) => {
			if (store === created) summary = s;
		},
		refused: (r) => {
			if (store === created) refusals.push(r);
		}
	});
	store = created;
});

describe('QueueStore', () => {
	it('loads the server queue when the connection says hello, holding no tracks', async () => {
		await hello();

		expect(summary).toMatchObject({
			queueId: QUEUE,
			context: CONTEXT,
			version: 0,
			current: 'a.0',
			manual: 0,
			upNext: 4,
			history: 0,
			repeat: 'off',
			shuffled: false
		});
		expect(server.requests[0].headers).not.toHaveProperty('If-None-Match');
	});

	it('says when the item playing came from the Manual Queue', async () => {
		await hello();
		expect(summary.playingManual).toBe(false);
		store.edit(queueManual('m'));
		store.edit(next);
		expect(summary).toMatchObject({ current: 'm.0', playingManual: true });
		store.edit(next);
		expect(summary).toMatchObject({ current: 'a.1', playingManual: false });
	});

	it('replays pending edits over edits from elsewhere, each applied once', async () => {
		await hello();
		store.edit(queueManual('m'));
		// At once, before anything is sent.
		expect(summary).toMatchObject({ manual: 1, upNext: 5, version: 0 });
		await settle();
		const ours = server.heardOf();

		// Another device's edit was applied first: the version gap says ours is second.
		store.message({
			type: 'queueEdit',
			queueId: QUEUE,
			version: 1,
			editId: crypto.randomUUID(),
			edit: { type: 'remove', items: ids('a', 4) }
		});
		expect(summary).toMatchObject({ manual: 1, upNext: 4, version: 1 });

		store.message({ ...ours, version: 2 });
		expect(summary).toMatchObject({ manual: 1, upNext: 4, version: 2 });
		// Retired: a later edit is not replayed over a queue that has ours.
		store.message({
			type: 'queueEdit',
			queueId: QUEUE,
			version: 3,
			editId: crypto.randomUUID(),
			edit: { type: 'remove', items: ids('a', 3) }
		});
		expect(summary).toMatchObject({ manual: 1, upNext: 3, version: 3 });
		expect(refusals).toEqual([]);
	});

	it('makes no request while working offline, and sends its changes in order once it is not', async () => {
		await hello();
		const before = server.requests.length;
		connectivity.setWorkOffline(true);
		store.edit(next);
		store.start({
			queueId: '55555555-5555-4555-8555-555555555555',
			context: CONTEXT,
			source: { key: '22222222-2222-4222-8222-222222222222', libraryId: 'lib', tracks: ['1', '2'] },
			start: 0
		});
		await wait(300);
		expect(server.requests).toHaveLength(before);
		expect(refusals).toEqual([]);

		connectivity.setWorkOffline(false);
		await settle();
		const posts = server.requests.filter((r) => r.method === 'POST').map((r) => r.path);
		expect(posts).toEqual(['/me/queue/edits', '/me/queue']);
		expect(refusals).toEqual([]);
	});

	it('sends a change that could not reach the server again after the log’s pause, not at once', async () => {
		await hello();
		server.failures = [0];
		store.edit(next);
		await wait(300);
		const sent = () => server.requests.filter((r) => r.path === '/me/queue/edits');
		expect(sent()).toHaveLength(1);
		await sender.flush();
		expect(sent()).toHaveLength(2);
	});

	it('asks for the queue once when it cannot be fetched, and again only once the server is heard from', async () => {
		await hello();
		server.failures = [0];
		await hello();
		await wait(300);
		expect(server.gets()).toBe(2);
		await hello();
		expect(server.gets()).toBe(3);
	});

	it('sends an edit once when its answer is lost', async () => {
		await hello();
		server.loseAnswers = 1;
		store.edit(next);
		await settle();
		// Sent again by the log, without waiting out its pause.
		await sender.flush();

		const sent = server.requests.filter((r) => r.path === '/me/queue/edits');
		expect(sent).toHaveLength(2);
		expect(sent[1].body.editId).toBe(sent[0].body.editId);
		expect(server.version).toBe(1);
		expect(server.queue.historyLen()).toBe(1);
		store.message(server.heardOf());
		expect(summary).toMatchObject({ current: 'a.1', history: 1, version: 1 });
	});

	it('refetches on version change only', async () => {
		await hello();
		const revision = summary.revision;
		await hello();

		expect(server.gets()).toBe(2);
		expect(server.queueGets()[1].headers?.['If-None-Match']).toBe(`"r1.${QUEUE}.0"`);
		expect(summary.revision).toBe(revision);

		// An edit this device missed, as after a dropped connection.
		server.apply(next);
		await hello();
		expect(server.gets()).toBe(3);
		expect(server.queueGets()[2].headers?.['If-None-Match']).toBe(`"r1.${QUEUE}.0"`);
		expect(summary).toMatchObject({ version: 1, current: 'a.1', history: 1 });

		// Heard live, an edit needs no fetch; a gap does.
		store.message(server.apply(next));
		expect(summary.version).toBe(2);
		expect(server.gets()).toBe(3);
		server.apply(next);
		store.message(server.apply(next));
		await settle();
		expect(server.gets()).toBe(4);
		expect(summary).toMatchObject({ version: 4, current: 'a.4' });
	});

	it('drops edits the server lost, and says so', async () => {
		await hello();
		store.edit(queueManual('m'));
		await settle();
		// The server answered version 1, but its `queueEdit` never arrived, and it then restarted
		// from a save that predates the edit.
		server.restart(albumQueue(), 0);
		await hello();

		expect(refusals).toEqual([
			{ code: 'lost', message: expect.any(String), edit: queueManual('m') }
		]);
		expect(summary).toMatchObject({ manual: 0, upNext: 4, version: 0 });
		// Not sent again, which could apply it twice.
		expect(server.requests.filter((r) => r.path === '/me/queue/edits')).toHaveLength(1);
	});

	it('keeps an edit the server answered and the new snapshot holds', async () => {
		await hello();
		store.edit(queueManual('m'));
		await settle();
		// Its `queueEdit` was lost with the connection; the snapshot has it.
		await hello();

		expect(refusals).toEqual([]);
		expect(summary).toMatchObject({ manual: 1, upNext: 5, version: 1 });
	});

	it('reverts an edit the server refuses, and says so', async () => {
		await hello();
		const move: SentEdit = {
			type: 'move',
			items: ids('a', 1),
			before: { type: 'before', item: 'a.3' }
		};
		// Removed elsewhere, unheard here: the edit looks fine on this device.
		const removal = server.apply({ type: 'remove', items: ids('a', 3) });
		store.edit(move);
		expect(summary.upNext).toBe(4);
		await settle();

		expect(refusals).toEqual([
			{ code: 'anchor_gone', message: 'Couldn’t change the queue. anchor_gone', edit: move }
		]);
		expect(summary.upNext).toBe(4);
		store.message(removal);
		expect(summary).toMatchObject({ upNext: 3, version: 1 });
	});

	it('play_gone reverts and says so: a rewind to a play another device removed', async () => {
		await hello();
		store.edit(next);
		store.edit(next);
		await settle();
		store.message(server.heardOf(0));
		store.message(server.heardOf(1));
		expect(summary).toMatchObject({ current: 'a.2', history: 2, version: 2 });
		const rewind: SentEdit = {
			type: 'rewind',
			play: { play: 0, item: 'a.0' },
			key: crypto.randomUUID()
		};
		// Removed elsewhere, unheard here: the rewind looks fine on this device.
		const removal = server.apply({ type: 'removePlays', plays: [{ play: 0, item: 'a.0' }] });
		store.edit(rewind);
		expect(summary).toMatchObject({ current: 'a.0', history: 0 });
		await settle();

		expect(refusals).toEqual([
			{ code: 'play_gone', message: 'Couldn’t change the queue. play_gone', edit: rewind }
		]);
		expect(summary).toMatchObject({ current: 'a.2', history: 2 });
		store.message(removal);
		expect(summary).toMatchObject({ current: 'a.2', history: 1, version: 3 });
	});

	it('refuses at once an edit the core refuses, and does not send it', async () => {
		await hello();
		const move: SentEdit = {
			type: 'move',
			items: ids('a', 1),
			before: { type: 'before', item: 'zzz.0' }
		};
		store.edit(move);
		await settle();

		expect(refusals).toMatchObject([{ code: 'anchor_gone', edit: move }]);
		expect(server.requests.filter((r) => r.path === '/me/queue/edits')).toEqual([]);
	});

	it("says so when another device's edit, heard first, takes a pending edit's anchor", async () => {
		await hello();
		const move: SentEdit = {
			type: 'move',
			items: ids('a', 1),
			before: { type: 'before', item: 'a.3' }
		};
		// Held back so the edit is still pending when the other edit arrives.
		server.failures = [0, 0];
		store.edit(move);
		store.message({
			type: 'queueEdit',
			queueId: QUEUE,
			version: 1,
			editId: crypto.randomUUID(),
			edit: { type: 'remove', items: ids('a', 3) }
		});

		expect(refusals).toMatchObject([{ code: 'anchor_gone', edit: move }]);
		expect(summary).toMatchObject({ upNext: 3, version: 1 });
		server.failures = [];
	});

	it('shows an add of a recipe at once as the rows the view counts, until the server says how many', async () => {
		await hello();
		const answer = gate();
		server.editGate = answer.wait;
		const add: SentEdit = {
			type: 'add',
			source: { key: 'q', libraryId: 'lib', recipe: { type: 'album', albumId: '9' } as never },
			before: { type: 'manualStart' }
		};
		store.edit(add, 3);
		expect(summary.manual).toBe(3);
		expect(upNext(4)).toEqual(['unknown', 'unknown', 'unknown', '101']);
		await settle();
		expect(summary.manual).toBe(3);

		// The server resolved two.
		store.message(server.heardOf());
		answer.open();
		await settle();
		expect(summary).toMatchObject({ manual: 2, version: 1 });
		expect(refusals).toEqual([]);
	});

	describe('an add of a recipe resolved on the device', () => {
		const add: SentEdit = {
			type: 'add',
			source: { key: 'q', libraryId: 'lib', recipe: { type: 'album', albumId: '9' } as never },
			before: { type: 'manualStart' }
		};

		it('shows all its rows at once, and keeps the list for a server that names the same hash', async () => {
			await hello();
			device.answer = { ids: ['7', '8'], hash: 'h-q' };
			server.editGate = gate().wait;
			store.edit(add, 2);
			await settle();
			expect(upNext(3)).toEqual(['7', '8', '101']);

			store.message(server.heardOf());
			expect(upNext(3), 'at once, with no download').toEqual(['7', '8', '101']);
			await settle();
			expect(server.listGets()).toEqual(['h']);
			expect(caches.hashes()).toEqual(['h']);
		});

		it('leaves an add of a search to the server unless the app counts itself offline', async () => {
			await hello();
			device.answer = { ids: ['7', '8'], hash: 'h-q' };
			const search = (key: string): SentEdit => ({
				...add,
				source: { key, libraryId: 'lib', recipe: { type: 'search', q: 'a' } }
			});
			store.edit(search('q'), 2);
			await settle();
			expect(device.asked).toEqual([]);

			store.edit(search('r'), 2, true);
			await settle();
			expect(device.asked).toHaveLength(1);
		});

		it("downloads the server's list when its hash differs", async () => {
			await hello();
			device.answer = { ids: ['7', '8'], hash: 'not-the-servers' };
			server.lists.set('h-q', ['5', '6']);
			store.edit(add, 2);
			await settle();
			store.message(server.heardOf());
			await settle();

			expect(upNext(3)).toEqual(['5', '6', '101']);
			expect(server.listGets()).toEqual(['h', 'h-q']);
		});
	});

	it('shows the rows of its own recipe add as unknown while the server cannot be reached', async () => {
		await hello();
		server.failures = Array(1000).fill(0);
		store.edit(
			{
				type: 'add',
				source: { key: 'q', libraryId: 'lib', recipe: { type: 'album', albumId: '9' } as never },
				before: { type: 'manualStart' }
			},
			3
		);
		await settle();

		// The add names its list once it is sent, so the player waits for it.
		expect(upNext(3)).toEqual(['unknown', 'unknown', 'unknown']);
	});

	it('sends an add of a recipe with no length in its turn and shows it when the server answers', async () => {
		await hello();
		const add: SentEdit = {
			type: 'add',
			source: { key: 'q', libraryId: 'lib', recipe: { type: 'album', albumId: '9' } as never },
			before: { type: 'manualEnd' }
		};
		store.edit(add);
		// The device does not know its length, so nothing shows; a later edit does, at once.
		expect(summary.manual).toBe(0);
		store.edit(queueManual('m'));
		expect(summary.manual).toBe(1);
		await settle();
		expect(
			server.requests.filter((r) => r.path === '/me/queue/edits').map((r) => r.body.edit)
		).toEqual([add, queueManual('m')]);

		store.message(server.heardOf(0));
		expect(summary).toMatchObject({ manual: 3, version: 1 });
		store.message(server.heardOf(1));
		expect(summary).toMatchObject({ manual: 3, version: 2 });
		expect(refusals).toEqual([]);
	});

	it('fetches the queue when another replaces it, but not for the one it holds', async () => {
		await hello();
		store.message({ type: 'queueReplaced', queueId: QUEUE });
		await settle();
		expect(server.gets()).toBe(1);

		server.queueId = '22222222-2222-4222-8222-222222222222';
		server.queue = albumQueue();
		server.queue.apply(JSON.stringify(next));
		store.message({ type: 'queueReplaced', queueId: server.queueId });
		await settle();
		expect(server.gets()).toBe(2);
		expect(summary).toMatchObject({ queueId: server.queueId, current: 'a.1', version: 0 });
	});

	it('drops edits for a queue that was replaced, and says so', async () => {
		await hello();
		server.failures = [0];
		store.edit(queueManual('m'));
		server.queueId = '22222222-2222-4222-8222-222222222222';
		store.message({ type: 'queueReplaced', queueId: server.queueId });
		await settle();
		// The queue is not fetched while the edit waits to be sent again.
		expect(server.gets()).toBe(1);
		await sender.flush();
		await settle();

		expect(refusals).toMatchObject([{ code: 'superseded', edit: queueManual('m') }]);
		expect(summary).toMatchObject({ queueId: server.queueId, manual: 0 });
	});

	it('restores a kept queue at once, where it was left, and sends it after the edits made on the one it replaces', async () => {
		await hello();
		const kept = '33333333-3333-4333-8333-333333333333';
		const left = albumQueue();
		left.apply(JSON.stringify(next));
		server.kept.set(kept, { queue: left, position: { item: 'a.1', positionMs: 42_000 } });
		server.failures = [0];
		store.edit(queueManual('m'));

		const result = await store.restore(kept);
		expect(result).toEqual({ ok: true, position: { item: 'a.1', positionMs: 42_000 } });
		// Shown at once; what a report may name waits for the server's answer.
		expect(summary).toMatchObject({ queueId: kept, current: 'a.1', version: 7, held: null });
		await sender.flush();
		await settle();
		expect(summary).toMatchObject({ queueId: kept, current: 'a.1', held: 'a.1' });
		// The edit reached the queue it was made on first, as it would have online.
		const posts = server.requests.filter((r) => r.method === 'POST').map((r) => r.path);
		expect(posts).toEqual(['/me/queue/edits', '/me/queue/edits', `/me/queues/${kept}/restore`]);
		expect(refusals).toEqual([]);
		// The server's queue is the device's: asked for once, it answers 304.
		const gets = server.gets();
		store.message({ type: 'queueReplaced', queueId: kept });
		await settle();
		expect(server.gets()).toBe(gets);
		// And its edits go to it.
		store.edit(queueManual('n'));
		await settle();
		expect(server.requests.at(-1)).toMatchObject({
			path: '/me/queue/edits',
			body: { queueId: kept }
		});
	});

	it('restores after a start that is not yet answered, so the start cannot replace it', async () => {
		await hello();
		const kept = '33333333-3333-4333-8333-333333333333';
		server.kept.set(kept, { queue: albumQueue(), position: { item: 'a.0', positionMs: 0 } });
		const held = gate();
		server.startGate = held.wait;
		store.start({
			queueId: '55555555-5555-4555-8555-555555555555',
			context: CONTEXT,
			source: {
				key: '22222222-2222-4222-8222-222222222222',
				libraryId: 'lib',
				recipe: { type: 'album', albumId: '1', sort: 'album', order: 'asc' }
			},
			start: 1,
			startTrackId: '201',
			total: 5,
			loaded: ['200', '201']
		});
		expect(await store.restore(kept)).toMatchObject({ ok: true });
		await settle();
		expect(server.requests.some((r) => r.path.endsWith('/restore'))).toBe(false);

		held.open();
		await sender.flush();
		await settle();
		const posts = server.requests.filter((r) => r.method === 'POST').map((r) => r.path);
		expect(posts).toEqual(['/me/queue', `/me/queues/${kept}/restore`]);
		expect(summary).toMatchObject({ queueId: kept });
	});

	it('keeps an edit heard while a restore is answered, and fetches only to hear 304', async () => {
		await hello();
		const kept = '33333333-3333-4333-8333-333333333333';
		server.kept.set(kept, { queue: albumQueue(), position: { item: 'a.0', positionMs: 0 } });
		const held = gate();
		server.restoreGate = held.wait;
		// Another device's edit lands as the restore is answered.
		server.afterRestore = () => {
			server.version = 8;
			server.queue.apply(JSON.stringify({ type: 'repeat', mode: 'all' }));
		};
		expect(await store.restore(kept)).toMatchObject({ ok: true });
		await settle();
		// As the server does: the queue is replaced before the restore is answered, and an edit follows.
		store.message({ type: 'queueReplaced', queueId: kept });
		store.message({
			type: 'queueEdit',
			queueId: kept,
			version: 8,
			editId: 'other',
			edit: { type: 'repeat', mode: 'all' }
		});
		held.open();
		await sender.flush();
		await settle();
		expect(summary).toMatchObject({ queueId: kept, version: 8, repeat: 'all' });
		expect(server.gets()).toBe(2);
	});

	it('says a queue the server no longer keeps is not restored, and keeps the queue it has', async () => {
		await hello();
		store.message(server.apply({ type: 'repeat', mode: 'all' }));
		const gone = '44444444-4444-4444-8444-444444444444';
		expect(await store.restore(gone)).toMatchObject({ ok: false, code: 'not_found' });
		await settle();
		expect(server.requests.filter((r) => r.path.endsWith('/restore'))).toEqual([]);
		expect(summary).toMatchObject({ queueId: QUEUE, current: 'a.0', version: 1, repeat: 'all' });
	});

	it('sends the restore after a start whose first try failed', async () => {
		await hello();
		const kept = '33333333-3333-4333-8333-333333333333';
		server.kept.set(kept, { queue: albumQueue(), position: { item: 'a.0', positionMs: 0 } });
		server.failures = [503];
		store.start({
			queueId: '55555555-5555-4555-8555-555555555555',
			context: CONTEXT,
			source: { key: '22222222-2222-4222-8222-222222222222', libraryId: 'lib', tracks: ['1', '2'] },
			start: 0
		});
		expect(await store.restore(kept)).toMatchObject({ ok: true });
		await sender.flush();
		const posts = server.requests.filter((r) => r.method === 'POST').map((r) => r.path);
		expect(posts).toEqual(['/me/queue', '/me/queue', `/me/queues/${kept}/restore`]);
	});

	it('answers a second restore busy while the first is under way', async () => {
		await hello();
		const kept = '33333333-3333-4333-8333-333333333333';
		server.kept.set(kept, { queue: albumQueue(), position: { item: 'a.0', positionMs: 0 } });
		const held = gate();
		server.keptGate = held.wait;
		const first = store.restore(kept);
		await settle();

		expect(await store.restore(kept)).toMatchObject({ ok: false, code: 'busy' });
		held.open();
		expect(await first).toMatchObject({ ok: true });
		await sender.flush();
		await settle();
		expect(server.requests.filter((r) => r.path.endsWith('/restore'))).toHaveLength(1);
	});

	it('logs no restore of a queue whose state it lacks while the app counts itself offline, and says so', async () => {
		await hello();
		const kept = '33333333-3333-4333-8333-333333333333';
		expect(await store.restore(kept, true)).toMatchObject({ ok: false, code: 'offline' });
		// What is done meanwhile stays on the queue it was made on, and nothing replaces it later.
		store.edit(next);
		await sender.flush();
		await settle();

		expect(server.requests.filter((r) => r.path.endsWith('/restore'))).toEqual([]);
		expect(await sender.pending()).toBe(0);
		expect(summary).toMatchObject({ queueId: QUEUE, current: 'a.1', version: 0 });
		expect(refusals).toEqual([]);
	});

	it('says a state that could not be had is unreachable, logs nothing, and lets the queue go on', async () => {
		await hello();
		const kept = '33333333-3333-4333-8333-333333333333';
		server.kept.set(kept, { queue: albumQueue(), position: { item: 'a.0', positionMs: 0 } });
		server.keptDown = true;
		expect(await store.restore(kept)).toMatchObject({ ok: false, code: 'unreachable' });
		expect(await sender.pending()).toBe(0);
		store.edit(next);
		await settle();
		expect(summary).toMatchObject({ queueId: QUEUE, current: 'a.1' });
		expect(refusals).toEqual([]);
		// Another restore may be asked for at once.
		server.keptDown = false;
		expect(await store.restore(kept)).toMatchObject({ ok: true });
	});

	it('fetches again a queue it could not fetch once the server answers a change', async () => {
		await hello();
		server.failures = [0];
		await hello();
		await wait(300);
		expect(server.gets()).toBe(2);
		// Nothing but the answer to an edit says the server is back.
		store.edit(next);
		await settle();
		expect(server.gets()).toBe(3);
	});

	it('has no queue when the server has none', async () => {
		await hello();
		server.failures = [404];
		await hello();

		expect(summary).toMatchObject({ queueId: null, current: null, upNext: 0 });
		store.edit(next);
		expect(refusals).toMatchObject([{ code: 'no_queue' }]);
	});

	it('fetches again when the queue is replaced by the one held while a fetch is under way', async () => {
		await hello();
		const other = '22222222-2222-4222-8222-222222222222';
		// Another device starts a queue, then restores this one, within the fetch's round trip.
		server.queueId = other;
		server.onGet = () => {
			server.onGet = null;
			server.queueId = QUEUE;
			store.message({ type: 'queueReplaced', queueId: QUEUE });
		};
		store.message({ type: 'queueReplaced', queueId: other });
		await settle();

		expect(server.gets()).toBe(3);
		expect(summary.queueId).toBe(QUEUE);
	});

	it('applies an edit heard while the queue is fetched, whether the answer is whole or 304', async () => {
		await hello();
		server.apply(next);
		// The snapshot is made at version 1; the next edit is heard before the store reads it.
		server.onGet = () => {
			server.onGet = null;
			store.message(server.apply(next));
		};
		await hello();
		expect(server.gets()).toBe(2);
		expect(summary).toMatchObject({ version: 2, current: 'a.2' });

		server.onGet = () => {
			server.onGet = null;
			store.message(server.apply(next));
		};
		await hello();
		expect(server.gets()).toBe(3);
		expect(summary).toMatchObject({ version: 3, current: 'a.3' });
	});

	it('drops an edit made while the replacing queue is fetched', async () => {
		await hello();
		server.queueId = '22222222-2222-4222-8222-222222222222';
		server.onGet = () => {
			server.onGet = null;
			store.edit(queueManual('m'));
		};
		store.message({ type: 'queueReplaced', queueId: server.queueId });
		await settle();

		// Told once, though the server, which it still reaches, refuses it too.
		expect(refusals).toMatchObject([{ code: 'superseded', edit: queueManual('m') }]);
		expect(summary).toMatchObject({ queueId: server.queueId, manual: 0 });
	});

	it('does not call an edit lost while a gap in versions is still to be fetched', async () => {
		await hello();
		store.edit(queueManual('m'));
		await settle();
		server.restart(albumQueue(), 0);
		let heardAtRefetch = -1;
		server.onGet = () => {
			// This answer is the restart's snapshot; an edit with a gap in versions is heard meanwhile.
			server.onGet = () => (heardAtRefetch = refusals.length);
			store.message({
				type: 'queueEdit',
				queueId: QUEUE,
				version: 5,
				editId: crypto.randomUUID(),
				edit: next
			});
		};
		await hello();

		expect(server.gets()).toBe(3);
		expect(heardAtRefetch).toBe(0);
		expect(refusals).toMatchObject([{ code: 'lost' }]);
	});

	it('does not fetch again after the server refuses the fetch', async () => {
		await hello();
		server.failures = [401];
		await hello();

		expect(server.gets()).toBe(2);
	});
});

/** The state of a queue with sources `keys`, each of two tracks and list `h<key>`, the first playing. */
function twoSourceState(...keys: string[]) {
	return Queue.fromJson(
		JSON.stringify({
			sources: keys.map((key) => ({ key, libraryId: 'lib', list: `h${key}`, length: 2 })),
			copies: [],
			manual: [],
			order: [{ view: { key: keys.at(-1) }, start: 0, len: 2 }],
			cursor: 1,
			current: `${keys.at(-1)}.0`,
			history: [],
			nextPlay: 0,
			shuffle: null,
			repeat: 'off',
			adjusted: false
		})
	);
}

describe('QueueStore lists and rows', () => {
	it('fills from pages then lists', async () => {
		const lists = gate();
		const rows = gate();
		server.listGate = lists.wait;
		server.rowsGate = rows.wait;
		await hello();
		expect(upNext()).toEqual(['unknown', 'unknown', 'unknown', 'unknown']);

		rows.open();
		await settle();
		expect(upNext()).toEqual(['101', '102', '103', '104']);
		expect(store.rows({ section: 'current', offset: 0, limit: 1 }).rows[0]).toMatchObject({
			item: 'a.0',
			track: '100',
			status: 'known'
		});
		// Up Next's first page, once, as History has nothing to show.
		expect(server.rowGets()).toEqual(['upNext:0']);

		lists.open();
		await settle();
		expect(upNext()).toEqual(['101', '102', '103', '104']);
		expect(server.listGets()).toEqual(['h']);
		expect(server.rowGets()).toEqual(['upNext:0']);
		expect(server.requests.some((r) => r.path.startsWith('/tracks'))).toBe(false);
	});

	it('asks again for a window only when the version changes, so never in a loop', async () => {
		server.rowsStatus = 500;
		server.listGate = new Promise(() => {});
		await hello();
		for (let i = 0; i < 5; i++) {
			upNext();
			await settle();
		}
		expect(server.rowGets()).toEqual(['upNext:0']);
		expect(upNext()).toEqual(['unknown', 'unknown', 'unknown', 'unknown']);

		// A new version asks again, for Up Next and now for History's first play too.
		store.message(server.apply(next));
		await settle();
		upNext();
		await settle();
		expect(server.rowGets().sort()).toEqual(['history:0', 'upNext:0', 'upNext:0']);
	});

	it('fetches a window past the first page a page at a time', async () => {
		server.listGate = new Promise(() => {});
		server.queue = Queue.fromJson(
			JSON.stringify({
				...JSON.parse(albumQueue().toJson()),
				sources: [{ key: 'a', libraryId: 'lib', list: 'h', length: 300 }],
				order: [{ view: { key: 'a' }, start: 0, len: 300 }]
			})
		);
		server.lists.set(
			'h',
			Array.from({ length: 300 }, (_, i) => String(1000 + i))
		);
		await hello();
		await settle();
		const window = store.rows({ section: 'upNext', offset: 150, limit: 20 });
		expect(window.total).toBe(299);
		expect(window.rows[0].status).toBe('unknown');
		await settle();

		expect(server.rowGets()).toContain('upNext:100');
		expect(store.rows({ section: 'upNext', offset: 150, limit: 20 }).rows[0]).toMatchObject({
			item: 'a.151',
			track: '1151'
		});
	});

	it('cleans Cache Storage on load, and attaches what it keeps without asking the server', async () => {
		caches.keep('h', ['100', '101', '102', '103', '104']);
		caches.keep('stale', ['1']);
		await hello();

		expect(caches.hashes()).toEqual(['h']);
		expect(upNext()).toEqual(['101', '102', '103', '104']);
		expect(server.listGets()).toEqual([]);
		expect(server.rowGets()).toEqual([]);
	});

	it('attaches its lists again after every install', async () => {
		await hello();
		expect(server.listGets()).toEqual(['h']);
		expect(caches.hashes()).toEqual(['h']);
		const pages = server.rowGets().length;

		// An edit missed, so the queue is fetched whole and a new core queue holds no list.
		server.apply(next);
		await hello();
		expect(summary.version).toBe(1);
		expect(upNext()).toEqual(['102', '103', '104']);
		expect(server.listGets()).toEqual(['h']);
		expect(server.rowGets()).toHaveLength(pages);
	});

	it('shares the lists it holds with a refetched queue instead of reading them again', async () => {
		caches.keep('h', ['100', '101', '102', '103', '104']);
		await hello();
		expect(caches.reads).toEqual(['h']);

		server.apply(next);
		await hello();
		expect(summary.version).toBe(1);
		expect(upNext()).toEqual(['102', '103', '104']);
		expect(caches.reads).toEqual(['h']);
		expect(server.listGets()).toEqual([]);
	});

	it('shows a list that failed once as unknown while a page for the row is on its way', async () => {
		server.listGate = Promise.reject(new TypeError('blip'));
		server.listGate.catch(() => {});
		const rows = gate();
		server.rowsGate = rows.wait;
		server.rowsStatus = 500;
		await hello();
		expect(server.listGets()).toEqual(['h']);
		expect(server.rowGets()).toContain('upNext:0');
		expect(store.rows({ section: 'current', offset: 0, limit: 1 }).rows[0].status).toBe('unknown');
		expect(upNext()).toEqual(['unknown', 'unknown', 'unknown', 'unknown']);

		// The page fails too, so nothing can still give the rows.
		rows.open();
		await settle();
		expect(store.rows({ section: 'current', offset: 0, limit: 1 }).rows[0].status).toBe(
			'unavailable'
		);
		expect(upNext()).toEqual(['unavailable', 'unavailable', 'unavailable', 'unavailable']);
	});

	it('marks the rows of a list the server does not have unavailable, and does not ask again', async () => {
		server.lists.delete('h');
		await hello();

		expect(upNext()).toEqual(['unavailable', 'unavailable', 'unavailable', 'unavailable']);
		await hello();
		server.apply(next);
		await hello();
		expect(server.listGets()).toEqual(['h']);
	});

	it('shows an unfetchable list unavailable even when its pages are down too', async () => {
		server.lists.delete('h');
		server.rowsStatus = 500;
		await hello();

		expect(upNext()).toEqual(['unavailable', 'unavailable', 'unavailable', 'unavailable']);
		expect(store.rows({ section: 'current', offset: 0, limit: 1 }).rows[0].status).toBe(
			'unavailable'
		);
	});

	it('asks again for a list that could not be reached when the connection is back', async () => {
		server.listGate = Promise.reject(new TypeError('offline'));
		server.listGate.catch(() => {});
		await hello();
		expect(upNext()).toEqual(['101', '102', '103', '104']);
		expect(server.listGets()).toEqual(['h']);

		server.listGate = null;
		await hello();
		expect(server.listGets()).toEqual(['h', 'h']);
	});

	it('loads first the lists that the item playing and Up Next read', async () => {
		server.queue = twoSourceState('z', 'a', 'b');
		for (const key of ['z', 'a', 'b']) server.lists.set(`h${key}`, ['1', '2']);
		await hello();
		await settle();

		expect(server.listGets()).toEqual(['hb', 'hz', 'ha']);
	});

	it('lets go of a list when clean-up drops it', async () => {
		await hello();
		store.edit({
			type: 'add',
			source: { key: 'm', libraryId: 'lib', tracks: ['7', '8'] },
			before: { type: 'manualEnd' }
		});
		await settle();
		store.message(server.heardOf());
		await settle();
		expect(caches.hashes().sort()).toEqual(['h', 'h-7-8']);
		// Its own tracks were never downloaded back.
		expect(server.listGets()).toEqual(['h']);
		expect(upNext()).toEqual(['7', '8', '101', '102', '103', '104']);

		store.edit({ type: 'remove', items: ids('m', 0, 1) });
		await settle();
		store.message(server.heardOf());
		await settle();
		expect(caches.hashes()).toEqual(['h']);

		// The same tracks added again are a list with the same hash, which must load afresh.
		store.edit({
			type: 'add',
			source: { key: 'n', libraryId: 'lib', tracks: ['7', '8'] },
			before: { type: 'manualEnd' }
		});
		await settle();
		store.message(server.heardOf());
		await settle();
		expect(upNext()).toEqual(['7', '8', '101', '102', '103', '104']);
		expect(caches.hashes().sort()).toEqual(['h', 'h-7-8']);
	});

	it('shows the tracks of its own add once the server has named their list, before it confirms', async () => {
		await hello();
		store.edit({
			type: 'add',
			source: { key: 'm', libraryId: 'lib', tracks: ['7', '8'] },
			before: { type: 'manualStart' }
		});
		await settle();

		expect(summary.manual).toBe(2);
		expect(upNext(2)).toEqual(['7', '8']);
		expect(server.listGets()).toEqual(['h']);
	});

	it('keeps lists in memory and uses pages when Cache Storage throws', async () => {
		caches.broken = true;
		server.listGate = new Promise(() => {});
		await hello();
		expect(upNext()).toEqual(['101', '102', '103', '104']);
		expect(server.rowGets()).toEqual(['upNext:0']);
	});

	it('keeps a list in memory when Cache Storage throws, so a reload of the queue does not download it again', async () => {
		caches.broken = true;
		await hello();
		expect(upNext()).toEqual(['101', '102', '103', '104']);

		server.apply(next);
		await hello();
		expect(upNext()).toEqual(['102', '103', '104']);
		expect(server.listGets()).toEqual(['h']);
	});

	it('works with no Cache Storage at all', async () => {
		store = new QueueStore({
			api: API,
			user: '7',
			fetch: server.fetch as typeof fetch,
			log: pageLog(server.fetch as typeof fetch, () => store).log,
			storage: undefined,
			changed: (s) => (summary = s),
			refused: (r) => refusals.push(r)
		});
		await hello();
		expect(upNext()).toEqual(['101', '102', '103', '104']);
	});

	it("fills the rows of its own add at once, before the server's answer", async () => {
		await hello();
		const answer = gate();
		server.editGate = answer.wait;
		store.edit({
			type: 'add',
			source: { key: 'm', libraryId: 'lib', tracks: ['7', '8'] },
			before: { type: 'manualStart' }
		});
		expect(upNext(3)).toEqual(['7', '8', '101']);
		await settle();
		expect(upNext(3)).toEqual(['7', '8', '101']);
		answer.open();
		await settle();
		expect(server.listGets()).toEqual(['h']);
	});

	it('shows every row of an add of several tracks before the server confirms it', async () => {
		await hello();
		const answer = gate();
		server.editGate = answer.wait;
		store.edit(queueManual('x'));
		store.edit({
			type: 'add',
			source: { key: 'm', libraryId: 'lib', tracks: ['7', '8', '9', '10'] },
			before: { type: 'manualEnd' }
		});
		// Behind a pending add of one, and before anything is sent or answered.
		expect(summary.manual).toBe(5);
		expect(upNext(7)).toEqual(['x-0', '7', '8', '9', '10', '101', '102']);
		// A move of the added rows keeps each its own track.
		store.edit({ type: 'move', items: ids('m', 3, 1), before: { type: 'manualStart' } });
		expect(upNext(5)).toEqual(['8', '10', 'x-0', '7', '9']);
		answer.open();
		await settle();
		expect(upNext(5)).toEqual(['8', '10', 'x-0', '7', '9']);
	});

	it('shows no row for an add of a recipe before the server resolves it', async () => {
		await hello();
		server.editGate = gate().wait;
		store.edit({
			type: 'add',
			source: {
				key: 'r',
				libraryId: 'lib',
				recipe: { type: 'album', albumId: '1', sort: 'album', order: 'asc' }
			},
			before: { type: 'manualStart' }
		});
		expect(summary.manual).toBe(0);
		expect(upNext(2)).toEqual(['101', '102']);
	});

	it('does not download its own add back when the edit is heard before it is answered', async () => {
		await hello();
		const answer = gate();
		server.editGate = answer.wait;
		store.edit({
			type: 'add',
			source: { key: 'm', libraryId: 'lib', tracks: ['7', '8'] },
			before: { type: 'manualStart' }
		});
		await settle();
		store.message(server.heardOf());
		answer.open();
		await settle();

		expect(server.listGets()).toEqual(['h']);
		expect(upNext(2)).toEqual(['7', '8']);
	});

	it('needs no page to confirm its own add', async () => {
		await hello();
		const pages = server.rowGets().length;
		store.edit({
			type: 'add',
			source: { key: 'm', libraryId: 'lib', tracks: ['7', '8'] },
			before: { type: 'manualStart' }
		});
		await settle();
		store.message(server.heardOf());
		await settle();

		expect(server.rowGets()).toHaveLength(pages);
		expect(upNext(2)).toEqual(['7', '8']);
	});

	it('shows the rows of a list that could not be reached as unavailable', async () => {
		server.listGate = Promise.reject(new TypeError('offline'));
		server.listGate.catch(() => {});
		server.rowsStatus = 500;
		await hello();

		expect(upNext()).toEqual(['unavailable', 'unavailable', 'unavailable', 'unavailable']);
	});

	it('shows a row the server could not read as unavailable', async () => {
		server.listGate = new Promise(() => {});
		server.lists.delete('h');
		await hello();

		expect(upNext()).toEqual(['unavailable', 'unavailable', 'unavailable', 'unavailable']);
	});

	it('does not ask again for a list that does not fit the queue', async () => {
		server.lists.set('h', ['1']);
		await hello();
		await settle();

		expect(server.listGets()).toEqual(['h']);
	});

	describe('a start', () => {
		const SOURCE = '22222222-2222-4222-8222-222222222222';
		const STARTED = '33333333-3333-4333-8333-333333333333';
		/** A start of a recipe over five tracks, its view having loaded `loaded` of them, clicking the second. */
		const recipeStart = (loaded = ['200', '201', '202']) => ({
			queueId: STARTED,
			context: CONTEXT,
			source: {
				key: SOURCE,
				libraryId: 'lib',
				recipe: {
					type: 'album' as const,
					albumId: '1',
					sort: 'album' as const,
					order: 'asc' as const
				}
			},
			start: 1,
			startTrackId: '201',
			total: 5,
			loaded
		});
		/** The current row, and the first rows of Up Next, as tracks or statuses. */
		const now = () => {
			const row = store.rows({ section: 'current', offset: 0, limit: 1 }).rows[0];
			return [row.track ?? row.status, ...upNext()];
		};

		it('plays the clicked row and fills its unshuffled rows from the view before the server answers', async () => {
			await hello();
			server.startGate = new Promise(() => {});
			expect(store.start(recipeStart())).toBe(true);

			expect(summary).toMatchObject({ queueId: STARTED, context: CONTEXT, current: `${SOURCE}.1` });
			expect(summary).toMatchObject({ upNext: 3, history: 0, version: 0 });
			// The view loaded three rows: what it did not load is waited for.
			expect(now()).toEqual(['201', '202', 'unknown', 'unknown']);
			expect(summary.held, 'the server has not got the queue yet').toBeNull();
			expect(server.requests.at(-1)).toMatchObject({ method: 'POST', path: '/me/queue' });
			expect(server.requests.at(-1)!.body).toEqual({
				queueId: STARTED,
				context: CONTEXT,
				source: recipeStart().source,
				start: 1,
				startTrackId: '201'
			});
		});

		it('keeps the repeat mode, here and on the server', async () => {
			await hello();
			store.edit({ type: 'repeat', mode: 'all' });
			await settle();
			store.start(recipeStart());
			expect(summary).toMatchObject({ queueId: STARTED, repeat: 'all' });
			await settle();

			expect(server.gets()).toBe(2);
			expect(summary).toMatchObject({ queueId: STARTED, repeat: 'all' });
		});

		it('waits for the rows a start made offline has not loaded instead of skipping them', async () => {
			await hello();
			server.failures = Array(1000).fill(0);
			store.start(recipeStart(['200', '201']));
			await settle();

			// The start names their list once it is sent, so the player waits for it.
			expect(now()).toEqual(['201', 'unknown', 'unknown', 'unknown']);
		});

		it('keeps the rows the view had not loaded unknown when the server accepts the start but its queue cannot be fetched', async () => {
			await hello();
			// The start is answered; every request after it fails.
			server.failures = [undefined as never, ...Array(1000).fill(0)];
			store.start(recipeStart(['200', '201']));
			await settle();

			expect(now()).toEqual(['201', 'unknown', 'unknown', 'unknown']);
		});

		it('sends a repeat change the start replaced before it was sent, so here and the server agree', async () => {
			await hello();
			const answer = gate();
			server.editGate = answer.wait;
			store.edit({ type: 'next' });
			await settle();
			// Waits behind the edit in flight.
			store.edit({ type: 'repeat', mode: 'all' });
			const { total, loaded, ...body } = recipeStart();
			void total;
			void loaded;
			store.start({
				...body,
				source: { key: SOURCE, libraryId: 'lib', tracks: ['200', '201', '202'] },
				startTrackId: undefined
			});
			expect(summary).toMatchObject({ queueId: STARTED, repeat: 'all' });
			answer.open();
			await settle();

			expect(summary).toMatchObject({ queueId: STARTED, repeat: 'all' });
			expect(JSON.parse(server.queue.toJson()).repeat).toBe('all');
			expect(refusals.map((r) => r.edit?.type)).not.toContain('repeat');
		});

		it("fetches the server's queue once it answers a recipe, and fills the rest from its list", async () => {
			await hello();
			store.start(recipeStart());
			await settle();

			expect(server.gets()).toBe(2);
			expect(server.queueGets().at(-1)!.headers).not.toHaveProperty('If-None-Match');
			expect(summary).toMatchObject({ queueId: STARTED, current: `${SOURCE}.1`, upNext: 3 });
			expect(summary.held, 'the server has it now').toBe(`${SOURCE}.1`);
			expect(now()).toEqual(['201', '202', '203', '204']);
			expect(server.listGets().at(-1)).toBe(`hs-${STARTED.slice(0, 4)}`);
		});

		it("replaces the view's rows with the server's when the library changed", async () => {
			await hello();
			server.resolved5 = ['200', '201', '302', '303', '304'];
			const held = gate();
			server.listGate = held.wait;
			store.start(recipeStart());
			await settle();

			// The list is on its way: the page the server gave says what its rows are.
			expect(now()).toEqual(['201', '302', '303', '304']);
			held.open();
		});

		/** The hash the server names for the recipe start's list. */
		const SERVER_LIST = `hs-${STARTED.slice(0, 4)}`;

		it('a matching hash downloads nothing', async () => {
			await hello();
			device.answer = { ids: server.resolved5, hash: SERVER_LIST };
			const pages = server.rowGets();
			const answer = gate();
			server.startGate = answer.wait;
			store.start(recipeStart(['200']));
			await settle();
			// Before the server has answered, the device's list is the queue's: all of it.
			expect(now()).toEqual(['201', '202', '203', '204']);
			expect(server.listGets()).toEqual(['h']);
			answer.open();
			await settle();

			expect(now()).toEqual(['201', '202', '203', '204']);
			expect(server.listGets()).toEqual(['h']);
			expect(server.rowGets()).toEqual(pages);
			// The account goes with it, so an owner signed in as another account does not answer.
			expect(device.asked).toEqual([
				{ library: 'lib', name: 'resolveRecipe', offline: false, user: '7' }
			]);
			expect(caches.hashes()).toEqual([SERVER_LIST]);
		});

		it("a mismatch downloads the server's list", async () => {
			await hello();
			device.answer = { ids: ['200', '201', '202', '203', '204'], hash: 'device-hash' };
			server.resolved5 = ['200', '201', '302', '303', '304'];
			const answer = gate();
			server.startGate = answer.wait;
			store.start(recipeStart(['200']));
			await settle();
			expect(now()).toEqual(['201', '202', '203', '204']);
			answer.open();
			await settle();

			expect(server.listGets().at(-1)).toBe(SERVER_LIST);
			expect(now()).toEqual(['201', '302', '303', '304']);
			expect(caches.hashes()).toEqual([SERVER_LIST]);
		});

		it("uses the device's list that answers after the server has named a matching one, downloading nothing", async () => {
			await hello();
			const slow = gate();
			device.gate = slow.wait;
			device.answer = { ids: server.resolved5, hash: SERVER_LIST };
			const held = gate();
			server.listGate = held.wait;
			store.start(recipeStart(['200']));
			await settle();
			expect(summary.held).toBe(`${SOURCE}.1`);
			slow.open();
			await settle();
			held.open();
			await settle();

			expect(now()).toEqual(['201', '202', '203', '204']);
			expect(caches.hashes()).toEqual([SERVER_LIST]);
		});

		it('ignores a device list that answers after another start', async () => {
			await hello();
			const slow = gate();
			device.gate = slow.wait;
			device.answer = { ids: server.resolved5, hash: 'device-hash' };
			store.start(recipeStart());
			store.start({
				...recipeStart(),
				queueId: QUEUE,
				source: { key: 'b', libraryId: 'lib', tracks: ['7', '8'] },
				startTrackId: undefined,
				start: 0
			});
			slow.open();
			await settle();

			expect(now()).toEqual(['7', '8']);
			expect(caches.hashes()).not.toContain('device-hash');
		});

		it('starts as without a catalog when the device cannot resolve the recipe', async () => {
			await hello();
			store.start(recipeStart());
			await settle();

			expect(now()).toEqual(['201', '202', '203', '204']);
			expect(server.listGets().at(-1)).toBe(SERVER_LIST);
			expect(refusals).toEqual([]);
		});

		it('a late device list with another hash never replaces the server list', async () => {
			await hello();
			const slow = gate();
			device.gate = slow.wait;
			device.answer = { ids: ['200', '201', '202', '203', '204'], hash: 'device-hash' };
			server.resolved5 = ['200', '201', '302', '303', '304'];
			const held = gate();
			server.listGate = held.wait;
			store.start(recipeStart(['200']));
			await settle();
			slow.open();
			await settle();
			held.open();
			await settle();

			expect(now()).toEqual(['201', '302', '303', '304']);
			expect(caches.hashes()).toEqual([SERVER_LIST]);
		});

		it('leaves a search to the server unless the app counts itself offline', async () => {
			await hello();
			device.answer = { ids: server.resolved5, hash: SERVER_LIST };
			const search = {
				...recipeStart(),
				source: { key: SOURCE, libraryId: 'lib', recipe: { type: 'search' as const, q: 'a' } }
			};
			store.start(search);
			await settle();
			expect(device.asked).toEqual([]);

			store.start({ ...search, queueId: QUEUE, offline: true });
			await settle();
			expect(device.asked).toHaveLength(1);
			expect(device.asked[0].offline).toBe(true);
		});

		it('sends no offline flag to the server', async () => {
			await hello();
			store.start({ ...recipeStart(), offline: true });
			await settle();
			const sent = server.requests.find((r) => r.method === 'POST' && r.path === '/me/queue')!;
			expect(sent.body).not.toHaveProperty('offline');
		});

		it('attaches the tracks it sent under the hash the server gives, downloading and fetching nothing', async () => {
			await hello();
			const pages = server.rowGets();
			const { total, loaded, ...body } = recipeStart(['200', '201', '202', '203', '204']);
			void total;
			void loaded;
			store.start({
				...body,
				source: { key: SOURCE, libraryId: 'lib', tracks: ['200', '201', '202', '203', '204'] },
				startTrackId: undefined
			});
			expect(now()).toEqual(['201', '202', '203', '204']);
			await settle();

			expect(now()).toEqual(['201', '202', '203', '204']);
			expect(server.gets()).toBe(1);
			expect(server.listGets()).toEqual(['h']);
			expect(server.rowGets()).toEqual(pages);
			expect(caches.hashes()).toEqual([`hs-${STARTED.slice(0, 4)}`]);
		});

		it('does not fill a shuffled queue from the view, but still the row playing', async () => {
			await hello();
			server.startGate = new Promise(() => {});
			store.start(recipeStart(['200', '201', '202', '203', '204']));
			store.edit({ type: 'shuffle', on: true, seed: 7 });

			expect(summary.shuffled).toBe(true);
			expect(now()).toEqual(['201', 'unknown', 'unknown', 'unknown']);
		});

		it('sends a start again when the server cannot be reached, and fills the rows once it answers', async () => {
			await hello();
			server.failures = [0];
			store.start(recipeStart());
			await Promise.resolve();
			await new Promise((resolve) => setTimeout(resolve, 0));

			// The start names their list once it arrives, so these wait.
			expect(now()).toEqual(['201', '202', 'unknown', 'unknown']);
			await settle();
			await sender.flush();
			await settle();
			// Sent again, and answered: the same start, not a second.
			const posts = server.requests.filter((r) => r.method === 'POST');
			expect(posts.length).toBe(2);
			expect(posts[1].body).toEqual(posts[0].body);
			expect(now()).toEqual(['201', '202', '203', '204']);
		});

		it('sends edits made before the answer after the start, to the new queue', async () => {
			await hello();
			const held = gate();
			server.startGate = held.wait;
			store.start(recipeStart());
			store.edit({ type: 'next' });
			expect(summary).toMatchObject({ current: `${SOURCE}.2`, history: 1 });
			held.open();
			await settle();

			const posts = server.requests.filter((r) => r.method === 'POST').map((r) => r.path);
			expect(posts.slice(0, 2)).toEqual(['/me/queue', '/me/queue/edits']);
			expect(server.requests.find((r) => r.path === '/me/queue/edits')!.body.queueId).toBe(STARTED);
			expect(summary).toMatchObject({ current: `${SOURCE}.2`, version: 1 });
			expect(refusals).toEqual([]);
		});

		it('takes back a start the server refuses, with the queue it has', async () => {
			await hello();
			server.failures = [422];
			store.start(recipeStart());
			store.edit({ type: 'next' });
			await settle();

			// The edit made on it goes with it, unsent, and the one refusal is the start's.
			expect(refusals).toMatchObject([{ code: 'failed', edit: null }]);
			expect(server.requests.filter((r) => r.path === '/me/queue/edits')).toEqual([]);
			expect(summary).toMatchObject({ queueId: QUEUE, current: 'a.0' });
		});

		it('is not undone by a fetch of the queue it replaced that was under way', async () => {
			server.startGate = new Promise(() => {});
			server.onGet = () => {
				server.onGet = null;
				store.start(recipeStart());
			};
			store.message({ type: 'hello', deviceId: '1', serverTime: '' });
			await settle();

			expect(summary.queueId).toBe(STARTED);
			expect(summary.current).toBe(`${SOURCE}.1`);
		});

		it('is not cleared by an answer that the account had no queue, after the start', async () => {
			server.startGate = new Promise(() => {});
			server.failures = [404];
			store.message({ type: 'hello', deviceId: '1', serverTime: '' });
			// The fetch is on its way.
			store.start(recipeStart());
			await settle();

			expect(summary).toMatchObject({ queueId: STARTED, current: `${SOURCE}.1` });
		});

		it('does not play a stale view row when the server moved the clicked track', async () => {
			await hello();
			// A track was added ahead of the clicked one since the view loaded.
			server.resolved5 = ['199', '200', '201', '202', '203'];
			server.listGate = new Promise(() => {});
			server.rowsGate = new Promise(() => {});
			store.start(recipeStart(['200', '201', '202']));
			await settle();

			expect(summary.current).toBe(`${SOURCE}.2`);
			// Not the view's third row, which is another track: it waits for the page.
			expect(now()[0]).toBe('unknown');
		});

		it('does not fill Up Next from the view when a track was added after the clicked one', async () => {
			await hello();
			server.resolved5 = ['200', '201', '299', '202', '203'];
			server.listGate = new Promise(() => {});
			server.rowsGate = new Promise(() => {});
			store.start({ ...recipeStart(['200', '201', '202', '203']), total: 4 });
			await settle();

			// The view's next row is another track than the server's, which the page will say.
			expect(now().slice(1, 2)).toEqual(['unknown']);
		});

		it('is not replaced by a snapshot that was being installed when the start was made', async () => {
			caches.keep('h', ['100', '101', '102', '103', '104']);
			const reading = gate();
			caches.matchGate = reading.wait;
			server.startGate = new Promise(() => {});
			store.message({ type: 'hello', deviceId: '1', serverTime: '' });
			await settle();
			store.start(recipeStart());
			reading.open();
			await settle();

			expect(summary).toMatchObject({ queueId: STARTED, current: `${SOURCE}.1` });
		});

		it('refuses a start at a row the source does not have', () => {
			expect(store.start({ ...recipeStart(), start: 5 })).toBe(false);
			expect(store.start({ ...recipeStart(), start: -1 })).toBe(false);
			expect(store.rows({ section: 'current', offset: 0, limit: 1 }).rows).toEqual([]);
		});
	});

	it('counts History and Up Next separately', async () => {
		await hello();
		store.edit(next);

		expect(store.rows({ section: 'history', offset: 0, limit: 10 }).total).toBe(1);
		expect(store.rows({ section: 'upNext', offset: 0, limit: 10 }).total).toBe(3);
	});

	it('keeps edits heard while it installs a snapshot', async () => {
		caches.keep('h', ['100', '101', '102', '103', '104']);
		const reading = gate();
		caches.matchGate = reading.wait;
		store.message({ type: 'hello', deviceId: '1', serverTime: '' });
		await settle();
		store.message(server.apply(next));
		reading.open();
		await settle();

		expect(summary).toMatchObject({ version: 1, current: 'a.1' });
		expect(server.gets()).toBe(1);
	});

	it('does not download its own add back when the echo is missed and the queue is fetched', async () => {
		await hello();
		store.edit({
			type: 'add',
			source: { key: 'm', libraryId: 'lib', tracks: ['7', '8'] },
			before: { type: 'manualStart' }
		});
		await settle();
		await hello();

		expect(summary.manual).toBe(2);
		expect(upNext(2)).toEqual(['7', '8']);
		expect(server.listGets()).toEqual(['h']);
	});
});

describe('QueueStore hidden tracks', () => {
	/** History's tracks as the store gives them. */
	const history = () =>
		store.rows({ section: 'history', offset: 0, limit: 10 }).rows.map((row) => row.track);
	/** The reads of Hidden Songs, by cursor. */
	const hiddenReads = () =>
		server.requests
			.filter((r) => r.path.startsWith('/libraries/lib/hidden-tracks'))
			.map((r) => new URL(r.path, 'http://x').searchParams.get('cursor'));

	it('leaves the rows of hidden tracks out of what it shows, and keeps them in the queue', async () => {
		server.hidden.set('lib', ['102', '104']);
		server.hiddenPage = 1;
		await hello();
		expect(hiddenReads()).toEqual([null, '1']);
		// a.0 (100) plays; Up Next is 101 to 104.
		expect(upNext()).toEqual(['101', '103']);
		expect(summary).toMatchObject({ manual: 0, upNext: 2 });
		const window = store.rows({ section: 'upNext', offset: 1, limit: 5 });
		expect([window.total, window.rows.map((row) => row.track)]).toEqual([2, ['103']]);
		expect(window.rows[0].item).toBe('a.3');

		// Unhidden, it is back where it was.
		store.hidden('lib', '102', false);
		expect(upNext()).toEqual(['101', '102', '103']);
		expect(summary.upNext).toBe(3);
		store.edit(next);
		store.edit(next);
		expect(summary.current).toBe('a.2');
		expect(upNext()).toEqual(['103']);
	});

	it('leaves them out of the Manual Queue and History, and their counts', async () => {
		await hello();
		store.edit({
			type: 'add',
			source: { key: 'm', libraryId: 'lib', tracks: ['300'] },
			before: { type: 'manualEnd' }
		});
		await settle();
		// The server's word on the add attaches its tracks.
		store.message(server.heardOf());
		store.hidden('lib', '300', true);
		expect(summary).toMatchObject({ manual: 0, upNext: 4 });
		expect(upNext()).toEqual(['101', '102', '103', '104']);

		store.edit(next);
		store.edit(next);
		store.hidden('lib', '100', true);
		// Played: a.0 (100), then m.0 (300), newest first; a.1 plays.
		expect(history()).toEqual([]);
		expect(summary.history).toBe(0);
		store.hidden('lib', '300', false);
		expect(history()).toEqual(['300']);
		expect(summary).toMatchObject({ history: 1, upNext: 3 });
	});

	it('takes what the page told over a read of Hidden Songs that may predate it', async () => {
		server.hidden.set('lib', ['102']);
		const held = gate();
		server.hiddenGate = held.wait;
		await hello();
		expect(upNext()).toEqual(['101', '102', '103', '104']);
		store.hidden('lib', '102', false);
		store.hidden('lib', '103', true);
		expect(upNext()).toEqual(['101', '102', '104']);
		held.open();
		await settle();
		expect(upNext()).toEqual(['101', '102', '104']);
	});

	it('reads Hidden Songs again on reconnecting, and not before', async () => {
		server.hidden.set('lib', ['101']);
		await hello();
		expect(upNext()).toEqual(['102', '103', '104']);
		store.hidden('lib', '103', true);
		// The queue moves on to a.1 (101), hidden, which the player passes over.
		store.edit(next);
		await settle();
		expect(upNext()).toEqual(['102', '104']);
		expect(hiddenReads()).toHaveLength(1);

		// What the server says then is the word, over what the page told before.
		server.hidden.set('lib', ['104']);
		await hello();
		expect(hiddenReads()).toHaveLength(2);
		expect(upNext()).toEqual(['102', '103']);
	});

	it('reads Hidden Songs again on reconnecting to a queue that did not change', async () => {
		server.hidden.set('lib', ['101']);
		await hello();
		expect(upNext()).toEqual(['102', '103', '104']);

		// Hidden on another device while the connection was down, so never told of here.
		server.hidden.set('lib', ['101', '103']);
		const fetched = server.queueGets().length;
		await hello();
		const answered = server.queueGets().slice(fetched);
		expect(answered.map((r) => r.headers?.['If-None-Match'])).toEqual([`"r1.${QUEUE}.0"`]);
		expect(hiddenReads()).toHaveLength(2);
		expect(upNext()).toEqual(['102', '104']);
	});
});

describe('QueueStore kept for the next time the app opens, its changes in the log', () => {
	const STARTED = '33333333-3333-4333-8333-333333333333';
	const THIRD = '44444444-4444-4444-8444-444444444444';
	/** A start of a track list, clicking its second. */
	const tracksStart = (queueId = STARTED) => ({
		queueId,
		context: CONTEXT,
		source: { key: `k-${queueId.slice(0, 4)}`, libraryId: 'lib', tracks: ['300', '301', '302'] },
		start: 1
	});

	/** The database every page shares, as the tabs of one browser do. */
	let env: Env;
	/** Whether the server can be reached. */
	let net: { up: boolean };
	/** Every page made, to close what a test leaves. */
	let made: { close: () => void }[];

	beforeEach(() => {
		env = memoryEnv();
		net = { up: false };
		made = [];
	});
	afterEach(() => made.forEach((one) => one.close()));

	/**
	 * One page for `user` over the shared database: its sender and its queue worker's store, which
	 * the store's database may stand in for (`database`). Closing it stops what it sends, as closing
	 * the page does, and frees its mutations for another page to adopt.
	 */
	function instance(user = '7', options: { database?: Env } = {}) {
		let alive = true;
		let latest!: Summary;
		let kept: KeptQueue[] | null = null;
		let told = 0;
		const refusals: Refusal[] = [];
		const notices: string[] = [];
		const fetcher = ((input: never, init?: RequestInit) =>
			!alive
				? new Promise(() => {})
				: net.up
					? server.fetch(input, init)
					: Promise.reject(new TypeError('offline'))) as typeof fetch;
		const page = pageLog(fetcher, () => (alive ? created : undefined), { env, user });
		const created: QueueStore = new QueueStore({
			api: API,
			user,
			fetch: fetcher,
			log: page.log,
			storage: caches as unknown as CacheStorage,
			database: options.database ?? env,
			changed: (summary) => (latest = summary),
			refused: (refusal) => refusals.push(refusal),
			told: (message) => notices.push(message),
			kept: (entries) => ((kept = entries), told++)
		});
		const one = {
			store: created,
			sender: page.sender,
			summary: () => latest,
			/** The kept queues as the page is told them, by ID. */
			kept: () => kept?.map(({ queueId }) => queueId) ?? null,
			keptEntries: () => kept,
			/** How many times the page was told the kept queues. */
			keptTold: () => told,
			refusals,
			notices,
			close: () => {
				alive = false;
				page.sender.stop();
				created.close();
			}
		};
		made.push(one);
		return one;
	}
	type Page = ReturnType<typeof instance>;

	/** The server can be reached and says hello to `one`, which wakes its sender as the page does. */
	async function connect(one: Page) {
		net.up = true;
		one.store.message({ type: 'hello', deviceId: '1', serverTime: '' });
		one.sender.wake();
		await settle();
		await one.sender.idle();
		await settle();
	}

	/** A page that has loaded the server's queue, which then cannot be reached. */
	async function loaded(user = '7') {
		const one = instance(user);
		await one.store.resume();
		await connect(one);
		net.up = false;
		return one;
	}

	/** The records the database holds for user 7. */
	const records = () => env.open('7').all<QueueRecord>('queue');
	/** The edits the server has been sent, resends included. */
	const sentEdits = () => server.requests.filter((r) => r.path === '/me/queue/edits');
	/** The starts, edits and restores the server was sent, as `path queueId`, in order. */
	const posts = () =>
		server.requests
			.filter((r) => r.method === 'POST')
			.map((r) => `${r.path} ${r.body?.queueId ?? ''}`.trim());

	it('shows the queue as left, with the edits made offline on top, when the app opens offline', async () => {
		const first = await loaded();
		first.store.edit(queueManual('m1'));
		first.store.edit(next);
		await settle();
		first.close();

		const second = instance();
		await second.store.resume();

		expect(second.summary()).toMatchObject({ queueId: QUEUE, current: 'm1.0', manual: 0 });
		expect(
			second.store.rows({ section: 'upNext', offset: 0, limit: 10 }).rows.map((row) => row.item)
		).toEqual(['a.1', 'a.2', 'a.3', 'a.4']);
		expect(
			second.store.rows({ section: 'upNext', offset: 0, limit: 10 }).rows.map((row) => row.track)
		).toEqual(['101', '102', '103', '104']);
		expect(second.refusals).toEqual([]);
	});

	it('sends the edits of two closed pages once each, in the order they were made', async () => {
		const first = await loaded();
		const second = await loaded();
		first.store.edit(queueManual('m1'));
		await settle();
		second.store.edit(queueManual('m2'));
		await settle();
		first.close();
		second.close();

		const third = instance();
		await third.store.resume();
		expect(third.summary().manual).toBe(2);
		await connect(third);

		expect(sentEdits().map((r) => r.body.edit.source.key)).toEqual(['m1', 'm2']);
		expect(server.version).toBe(2);
		expect(await unsent(env.open('7'))).toBe(0);
		// Hearing hello again sends nothing twice.
		await connect(third);
		expect(sentEdits()).toHaveLength(2);
	});

	it('sends a start made offline, before the edits made on it, when the connection is back', async () => {
		const first = await loaded();
		expect(first.store.start({ ...tracksStart(), loaded: ['300'] })).toBe(true);
		first.store.edit(queueManual('m1'));
		await settle();
		await first.store.save();
		// What the server confirmed is all the record holds.
		expect(await records()).toEqual([expect.objectContaining({ queueId: QUEUE })]);
		first.close();

		const second = instance();
		await second.store.resume();
		expect(second.summary()).toMatchObject({ queueId: STARTED, current: 'k-3333.1', manual: 1 });
		expect(second.store.rows({ section: 'current', offset: 0, limit: 1 }).rows[0].track).toBe(
			'301'
		);
		expect(server.queueId).toBe(QUEUE);
		await connect(second);

		expect(posts()).toEqual([`/me/queue ${STARTED}`, `/me/queue/edits ${STARTED}`]);
		expect(server.queueId).toBe(STARTED);
		expect(server.version).toBe(1);
		expect(second.summary()).toMatchObject({ queueId: STARTED, version: 1, manual: 1 });
		await second.store.save();
		expect(await records()).toEqual([expect.objectContaining({ queueId: STARTED })]);
	});

	it('sends the edits made on a queue before the starts that replaced it offline, so the server keeps it with them', async () => {
		const one = await loaded();
		one.store.edit(next);
		one.store.edit({ type: 'move', items: ids('a', 3), before: { type: 'before', item: 'a.2' } });
		one.store.start(tracksStart());
		one.store.edit({ type: 'remove', items: ['k-3333.2'] });
		one.store.start(tracksStart(THIRD));
		await settle();
		expect(one.summary()).toMatchObject({ queueId: THIRD, current: 'k-4444.1' });
		expect(server.requests.filter((r) => r.method === 'POST')).toEqual([]);

		await connect(one);
		expect(posts()).toEqual([
			`/me/queue/edits ${QUEUE}`,
			`/me/queue/edits ${QUEUE}`,
			`/me/queue ${STARTED}`,
			`/me/queue/edits ${STARTED}`,
			`/me/queue ${THIRD}`
		]);
		// Both replaced queues were adjusted, so the server parked them as kept.
		expect(server.parked).toEqual([QUEUE, STARTED]);
		expect(server.queueId).toBe(THIRD);
		expect(one.refusals).toEqual([]);
		expect(one.summary()).toMatchObject({ queueId: THIRD, current: 'k-4444.1' });
	});

	it('leaves a queue that was not adjusted to be replaced, not kept, as the server does', async () => {
		const one = await loaded();
		one.store.edit(next);
		one.store.start(tracksStart());
		one.store.start(tracksStart(THIRD));
		await settle();
		await connect(one);

		expect(posts()).toEqual([
			`/me/queue/edits ${QUEUE}`,
			`/me/queue ${STARTED}`,
			`/me/queue ${THIRD}`
		]);
		// Skipping a song adjusts nothing, and nothing was done to the second.
		expect(server.parked).toEqual([]);
	});

	it("sends a closed page's edit once however often hello comes, and leaves a live page's to it", async () => {
		const closed = await loaded();
		closed.store.edit(queueManual('m1'));
		await settle();
		closed.close();
		const live = await loaded();
		live.store.edit(queueManual('m2'));
		await settle();

		const third = instance();
		await third.store.resume();
		await connect(third);
		await connect(third);
		await connect(live);

		expect(sentEdits().filter((r) => r.body.edit.source.key === 'm1')).toHaveLength(1);
		expect(sentEdits().filter((r) => r.body.edit.source.key === 'm2')).toHaveLength(1);
	});

	it('shows an edit of another page once, when the server has it and it was shown from the log', async () => {
		const live = await loaded();
		live.store.edit(queueManual('m1'));
		await settle();
		const second = instance();
		await second.store.resume();
		expect(second.summary().manual).toBe(1);

		await connect(live);
		await connect(second);
		expect(server.version).toBe(1);
		expect(second.summary()).toMatchObject({ version: 1, manual: 1 });
		expect(second.refusals).toEqual([]);
	});

	it('keeps showing a queue started offline, reopened and then reconnected, until the server has it', async () => {
		const first = await loaded();
		first.store.start(tracksStart());
		first.store.edit(queueManual('m1'));
		await settle();
		first.close();
		const second = instance();
		const shown: (string | null)[] = [];
		await second.store.resume();
		expect(second.summary().queueId).toBe(STARTED);
		const sample = setInterval(() => shown.push(second.summary().queueId), 1);

		// The server says hello before the page's sender has sent the start, and tells of nothing.
		net.up = true;
		second.store.message({ type: 'hello', deviceId: '1', serverTime: '' });
		await settle();
		second.sender.wake();
		await settle();
		await second.sender.idle();
		await settle();
		clearInterval(sample);

		expect(shown).not.toContain(QUEUE);
		expect(server.queueId).toBe(STARTED);
		expect(second.summary()).toMatchObject({ queueId: STARTED, version: 1, manual: 1 });
		expect(second.refusals).toEqual([]);
	});

	it('fetches once the server has what it showed from the log, though told while it read the log', async () => {
		const live = await loaded();
		live.store.start(tracksStart());
		await settle();
		/** Once armed, the next read of the log answers what it held then, only once let go. */
		let hold: Promise<void> | null = null;
		const slow: Env = {
			...env,
			open: (user) => {
				const port = env.open(user);
				return {
					...port,
					all: async <T extends Stored>(store: StoreName) => {
						const read = await port.all<T>(store);
						const held = store === 'mutations' ? hold : null;
						if (held) {
							hold = null;
							await held;
						}
						return read;
					}
				};
			}
		};
		const second = instance('7', { database: slow });
		await second.store.resume();
		expect(second.summary()).toMatchObject({ queueId: STARTED, held: null });
		const read = gate();
		hold = read.wait;
		net.up = true;
		second.store.message({ type: 'hello', deviceId: '1', serverTime: '' });
		await settle();

		// The live page sends its start while the read is held, and the server tells every page.
		await connect(live);
		expect(server.queueId).toBe(STARTED);
		second.store.message({ type: 'queueReplaced', queueId: STARTED });
		read.open();
		await settle();

		// Fetched: the server's queue is held, with the item it has playing.
		expect(second.summary()).toMatchObject({ queueId: STARTED, held: 'k-3333.1' });
	});

	it("sends an edit of a page that closed meanwhile before this one's later edit", async () => {
		const first = await loaded();
		const second = await loaded();
		first.store.edit(queueManual('m1'));
		await settle();
		second.store.edit(queueManual('m2'));
		first.close();
		await settle();
		await connect(second);

		expect(sentEdits().map((r) => r.body.edit.source.key)).toEqual(['m1', 'm2']);
	});

	it("keeps each user's queue apart", async () => {
		const first = await loaded('7');
		first.store.edit(queueManual('m1'));
		await first.store.save();
		first.close();

		const other = instance('8');
		await other.store.resume();

		expect(other.summary()).toBeUndefined();
		expect(await env.open('8').all('queue')).toEqual([]);
		expect(await records()).toHaveLength(1);
	});

	it('counts its starts and edits among the unsent, which the log holds', async () => {
		const first = await loaded();
		first.store.start(tracksStart());
		first.store.edit(queueManual('m1'));
		first.store.edit(next);
		await settle();

		expect(await unsent(env.open('7'))).toBe(3);
	});

	it('writes the record for what the server confirmed alone, and no more once the database is wiped', async () => {
		const puts: string[] = [];
		const counting: Env = {
			...env,
			open: (user) => {
				const port = env.open(user);
				return {
					...port,
					put: (store, record, strict) => (puts.push(store), port.put(store, record, strict))
				};
			}
		};
		const one = instance('7', { database: counting });
		await one.store.resume();
		await connect(one);
		expect(puts).toEqual(['queue']);
		net.up = false;
		// Edits are the log's, and a change of rows is nobody's.
		one.store.edit(queueManual('m1'));
		one.store.hidden('lib', '101', true);
		await settle();
		expect(puts).toEqual(['queue']);
		// What the server confirms is written.
		one.store.message(server.apply(next));
		await settle();
		expect(puts).toEqual(['queue', 'queue']);

		const wipe = new BroadcastChannel('jewelcase-outbox-7');
		wipe.postMessage('wipe');
		wipe.close();
		await settle();
		one.store.message(server.apply(next));
		await one.store.save();
		expect(puts).toEqual(['queue', 'queue']);
	});

	it('keeps an edit made offline just before the page closes', async () => {
		const one = await loaded();
		one.store.edit(queueManual('m1'));
		// Closed with no wait for a timer: the log wrote it as it was made.
		await new Promise((resolve) => setTimeout(resolve, 0));
		one.close();

		const again = instance();
		await again.store.resume();
		expect(again.summary().manual).toBe(1);
	});

	it('shows and sends a queue started offline in a page, though another page saved later', async () => {
		const starter = await loaded();
		const idle = await loaded();
		starter.store.start(tracksStart());
		await settle();
		await idle.store.save();
		starter.close();
		idle.close();

		const again = instance();
		await again.store.resume();
		expect(again.summary().queueId).toBe(STARTED);
		await connect(again);

		expect(server.queueId).toBe(STARTED);
	});

	it("sends a closed page's start before the edits made on it, from a live page's log", async () => {
		const starter = await loaded();
		const live = await loaded();
		starter.store.start(tracksStart());
		starter.store.edit(queueManual('m1'));
		await settle();
		starter.close();
		await connect(live);

		expect(server.queueId).toBe(STARTED);
		expect(posts()).toEqual([`/me/queue ${STARTED}`, `/me/queue/edits ${STARTED}`]);
	});

	it('sends the starts two pages made in the order they were made, the later active', async () => {
		const first = await loaded();
		const second = await loaded();
		first.store.start(tracksStart());
		await new Promise((resolve) => setTimeout(resolve, 5));
		second.store.start(tracksStart(THIRD));
		await settle();
		first.close();
		second.close();

		const again = instance();
		await again.store.resume();
		expect(again.summary().queueId).toBe(THIRD);
		await connect(again);

		expect(posts()).toEqual([`/me/queue ${STARTED}`, `/me/queue ${THIRD}`]);
		expect(again.refusals).toEqual([]);
		expect(again.notices).toEqual([]);
	});

	it('does not tell the page a failed write as a refused start, but once, plainly', async () => {
		const broken: Env = {
			...env,
			open: (user) => ({ ...env.open(user), put: () => Promise.reject(new Error('quota')) })
		};
		const one = instance('7', { database: broken });
		await one.store.resume();
		await connect(one);
		one.store.message(server.apply(next));
		one.store.edit(queueManual('m1'));
		await settle();

		expect(one.refusals).toEqual([]);
		expect(one.notices).toHaveLength(1);
	});

	it('asks with the version it kept and keeps that queue when the server says 304', async () => {
		const first = await loaded();
		await first.store.save();
		first.close();
		const again = instance();
		await again.store.resume();
		const before = server.gets();
		await connect(again);

		expect(
			server
				.queueGets()
				.slice(before)
				.map((r) => r.headers?.['If-None-Match'])
		).toEqual([`"r1.${QUEUE}.0"`]);
		expect(again.summary()).toMatchObject({ queueId: QUEUE, version: 0 });
	});

	it('reopens a recipe start with the rows its view had loaded', async () => {
		const first = await loaded();
		first.store.start({
			queueId: STARTED,
			context: CONTEXT,
			source: {
				key: 'rec',
				libraryId: 'lib',
				recipe: { type: 'album', albumId: '1', sort: 'album', order: 'asc' }
			},
			start: 1,
			startTrackId: '201',
			total: 5,
			loaded: ['200', '201', '202']
		});
		await settle();
		first.close();

		const again = instance();
		await again.store.resume();
		const rows = again.store.rows({ section: 'upNext', offset: 0, limit: 10 }).rows;
		expect(rows.map((row) => row.track ?? row.status)).toEqual(['202', 'unknown', 'unknown']);
		expect(again.store.rows({ section: 'current', offset: 0, limit: 1 }).rows[0].track).toBe('201');
	});

	it('shows the queue as last confirmed, not as a page that heard less wrote it', async () => {
		const behind = await loaded();
		const ahead = await loaded();
		ahead.store.message(server.apply(next));
		await settle();
		// The page that heard nothing is hidden later: its queue has not changed, so it is not written.
		await behind.store.save();
		behind.close();
		ahead.close();

		const again = instance();
		await again.store.resume();
		expect(again.summary()).toMatchObject({ version: 1, current: 'a.1' });
	});

	it('removes the records an earlier build kept for each instance', async () => {
		await env.open('7').put('queue', {
			key: 'instance',
			rev: 'r',
			owner: 'instance',
			createdAt: 1,
			tries: 0
		});
		const one = await loaded();
		await one.store.save();
		expect((await records()).map((record) => record.key)).toEqual(['queue']);
	});

	it('sends nothing of the ended account after the database is wiped, however it is woken', async () => {
		const one = await loaded();
		one.store.start(tracksStart());
		one.store.edit(queueManual('m1'));
		await settle();

		const wipe = new BroadcastChannel('jewelcase-outbox-7');
		wipe.postMessage('wipe');
		wipe.close();
		await settle();
		await connect(one);
		one.store.edit(next);
		await settle();

		expect(server.requests.filter((r) => r.method === 'POST')).toEqual([]);
		expect(server.queueId).toBe(QUEUE);
	});

	describe('kept queues on the device', () => {
		const K1 = '66666666-6666-4666-8666-666666666666';
		const K2 = '77777777-7777-4777-8777-777777777777';
		/** A kept queue of the album, its third song moved before its second, which adjusts it. */
		const adjustedAlbum = () => {
			const queue = albumQueue();
			queue.apply(
				JSON.stringify({ type: 'move', items: ['a.2'], before: { type: 'before', item: 'a.1' } })
			);
			return queue;
		};
		/** The edit that adjusts the queue shown, as `adjustedAlbum`'s. */
		const adjust: QueueEdit = {
			type: 'move',
			items: ['a.2'],
			before: { type: 'before', item: 'a.1' }
		};

		it('keeps a queue replaced offline at once, with its changes, and replaces one never adjusted', async () => {
			const one = await loaded();
			expect(one.kept()).toEqual([]);
			one.store.edit(adjust);
			one.store.start({ ...tracksStart(), parked: { item: 'a.0', positionMs: 5000 } });
			await settle();
			expect(one.kept()).toEqual([QUEUE]);
			expect(one.keptEntries()![0]).toMatchObject({
				current: { libraryId: 'lib', trackId: '100' },
				upNext: 4,
				history: 0
			});
			// The second, never adjusted, is replaced, not kept.
			one.store.start(tracksStart(THIRD));
			await settle();
			expect(one.kept()).toEqual([QUEUE]);

			// The server keeps the same, and its entry replaces the device's.
			await connect(one);
			expect(server.parked).toEqual([QUEUE]);
			expect(one.kept()).toEqual([QUEUE]);
			expect(one.refusals).toEqual([]);
		});

		it('restores a queue kept on the device offline, at once, and the edits made after it reach it on reconnect', async () => {
			server.kept.set(K1, { queue: adjustedAlbum(), position: { item: 'a.0', positionMs: 9000 } });
			const one = await loaded();
			expect(one.kept()).toEqual([K1]);
			connectivity.setWorkOffline(true);

			const result = await one.store.restore(K1, true, { item: 'a.0', positionMs: 1000 });
			expect(result).toEqual({ ok: true, position: { item: 'a.0', positionMs: 9000 } });
			expect(one.summary()).toMatchObject({ queueId: K1, current: 'a.0', held: null });
			expect(
				one.store.rows({ section: 'upNext', offset: 0, limit: 5 }).rows.map((r) => r.track)
			).toEqual(['102', '101', '103', '104']);
			// The restored queue is not listed, and the one it replaced, never adjusted, is not kept.
			expect(one.kept()).toEqual([]);
			one.store.edit(queueManual('x'));
			await settle();
			expect(server.requests.filter((r) => r.method === 'POST')).toEqual([]);

			connectivity.setWorkOffline(false);
			await connect(one);
			expect(posts()).toEqual([`/me/queues/${K1}/restore`, `/me/queue/edits ${K1}`]);
			expect(server.queueId).toBe(K1);
			expect(server.queue.manualLen()).toBe(1);
			expect(one.summary()).toMatchObject({ queueId: K1, manual: 1, held: 'a.0' });
			expect(one.refusals).toEqual([]);
			expect(one.kept()).toEqual([]);
		});

		it('lists and restores the kept queues when the app opens offline', async () => {
			server.kept.set(K1, { queue: adjustedAlbum(), position: null });
			server.kept.set(K2, { queue: albumQueue(), position: null });
			(await loaded()).close();

			const again = instance();
			await again.store.resume();
			expect(new Set(again.kept())).toEqual(new Set([K1, K2]));
			expect(await again.store.restore(K1, true)).toMatchObject({ ok: true });
			expect(
				again.store.rows({ section: 'upNext', offset: 0, limit: 5 }).rows.map((r) => r.track)
			).toEqual(['102', '101', '103', '104']);
			expect(again.kept()).toEqual([K2]);
		});

		/** A kept state over sources `b` and `c`, of lists `h2` and `h3`, adjusted, `b.0` playing. */
		const twoLists = () =>
			Queue.fromJson(
				JSON.stringify({
					sources: [
						{ key: 'b', libraryId: 'lib', list: 'h2', length: 5 },
						{ key: 'c', libraryId: 'lib', list: 'h3', length: 5 }
					],
					copies: [],
					manual: [],
					order: [
						{ view: { key: 'b' }, start: 0, len: 5 },
						{ view: { key: 'c' }, start: 0, len: 5 }
					],
					cursor: 1,
					current: 'b.0',
					history: [],
					nextPlay: 0,
					shuffle: null,
					repeat: 'off',
					adjusted: true
				})
			);
		/** An add of track `track` to the Manual Queue as source `key`. */
		const addOf = (key: string, track: string): SentEdit => ({
			type: 'add',
			source: { key, libraryId: 'lib', tracks: [track] },
			before: { type: 'manualEnd' }
		});
		/** The tracks of the first `n` rows of Up Next of `one`. */
		const tracksOf = (one: Page, n = 9) =>
			one.store.rows({ section: 'upNext', offset: 0, limit: n }).rows.map((r) => r.track);
		const TWO_LISTS = ['201', '202', '203', '204', '300', '301', '302', '303', '304'];

		it("keeps a kept queue's lists that an install meanwhile would let go of, while its state is fetched", async () => {
			server.lists.set('h2', ['200', '201', '202', '203', '204']);
			server.lists.set('h3', ['300', '301', '302', '303', '304']);
			server.kept.set(K1, { queue: twoLists(), position: null });
			const third = gate();
			server.listHolds.set('h3', third.wait);
			const one = instance();
			await one.store.resume();
			await connect(one);
			expect(server.listGets()).toContain('h3');

			// Another device starts a queue, which this one installs while h3 is on its way.
			const other = '88888888-8888-4888-8888-888888888888';
			server.start({
				queueId: other,
				context: CONTEXT,
				source: { key: 'z', libraryId: 'lib', tracks: ['1', '2'] },
				start: 0
			});
			one.store.message({ type: 'queueReplaced', queueId: other });
			await settle();
			expect(one.summary()).toMatchObject({ queueId: other });
			third.open();
			await settle();
			expect(caches.hashes()).toEqual(expect.arrayContaining(['h2', 'h3']));

			net.up = false;
			expect(await one.store.restore(K1, true)).toMatchObject({ ok: true });
			expect(tracksOf(one)).toEqual(TWO_LISTS);
		});

		it("keeps a kept queue's own lists through the install as the app opens", async () => {
			server.lists.set('h2', ['200', '201', '202', '203', '204']);
			server.lists.set('h3', ['300', '301', '302', '303', '304']);
			server.kept.set(K1, { queue: twoLists(), position: null });
			(await loaded()).close();

			const again = instance();
			await again.store.resume();
			expect(await again.store.restore(K1, true)).toMatchObject({ ok: true });
			expect(tracksOf(again)).toEqual(TWO_LISTS);
		});

		it('keeps a list a kept queue shares with the queue shown when another start replaces it', async () => {
			server.kept.set(K1, { queue: adjustedAlbum(), position: null });
			const one = await loaded();
			// The queue shown, of list h, is not adjusted, so it is replaced and lets go of h.
			one.store.start(tracksStart());
			await settle();
			expect(caches.hashes()).toContain('h');
			expect(await one.store.restore(K1, true)).toMatchObject({ ok: true });
			expect(tracksOf(one, 4)).toEqual(['102', '101', '103', '104']);
		});

		it('shows a restore made offline again when the app reopens, with the edits made on it, and sends them on reconnect', async () => {
			server.kept.set(K1, { queue: adjustedAlbum(), position: null });
			const one = await loaded();
			expect(await one.store.restore(K1, true)).toMatchObject({ ok: true });
			one.store.edit(queueManual('x'));
			await settle();
			one.close();

			const again = instance();
			await again.store.resume();
			expect(again.summary()).toMatchObject({ queueId: K1, manual: 1, held: null });
			expect(again.kept()).toEqual([]);
			await connect(again);
			expect(posts()).toEqual([`/me/queues/${K1}/restore`, `/me/queue/edits ${K1}`]);
			expect(again.summary()).toMatchObject({ queueId: K1, manual: 1, held: 'a.0' });
			expect(again.refusals).toEqual([]);
		});

		it('restores offline a queue kept with an add of its own, with the tracks it added', async () => {
			const one = await loaded();
			one.store.edit(addOf('m', '555'));
			one.store.start(tracksStart());
			await settle();
			expect(one.kept()).toEqual([QUEUE]);
			expect(await one.store.restore(QUEUE, true)).toMatchObject({ ok: true });
			expect(tracksOf(one, 1)).toEqual(['555']);
		});

		it("asks for a kept queue's state again when the server says it was active since", async () => {
			const at = (s: number) => new Date(server.keptSince + s * 1000).toISOString();
			server.kept.set(K1, { queue: adjustedAlbum(), position: null, at: at(1) });
			const one = await loaded();
			const asked = () => server.requests.filter((r) => r.path === `/me/queues/${K1}`).length;
			expect(asked()).toBe(1);
			net.up = true;
			one.store.message({ type: 'queuesChanged' });
			await settle();
			expect(asked()).toBe(1);
			server.kept.get(K1)!.at = at(2);
			one.store.message({ type: 'queuesChanged' });
			await settle();
			expect(asked()).toBe(2);
		});

		it('reads an unchanged list as one answer with no body, and tells the page once, not once per state', async () => {
			for (let i = 0; i < 20; i++)
				server.kept.set(`9999${String(i).padStart(4, '0')}-9999-4999-8999-999999999999`, {
					queue: adjustedAlbum(),
					position: null
				});
			const one = instance();
			await one.store.resume();
			const before = one.keptTold();
			await connect(one);
			await settle();
			expect(one.kept()).toHaveLength(20);
			expect(server.requests.filter((r) => /^\/me\/queues\/[^?]+$/.test(r.path))).toHaveLength(20);
			// Told when the list was read, not again for each of the twenty states.
			expect(one.keptTold() - before).toBeLessThanOrEqual(2);

			const reads = () => server.requests.filter((r) => r.path.startsWith('/me/queues?'));
			one.store.message({ type: 'queuesChanged' });
			await settle();
			expect(reads().at(-1)?.headers?.['If-None-Match']).toMatch(/^"k/);
			expect(one.kept()).toHaveLength(20);
		});

		it("keeps a queue parked here by a start not yet sent over the server's older entry", async () => {
			server.kept.set(K1, { queue: adjustedAlbum(), position: null });
			const one = await loaded();
			expect(await one.store.restore(K1, true)).toMatchObject({ ok: true });
			one.store.edit(addOf('x', '555'));
			one.store.start(tracksStart());
			await settle();
			expect(one.keptEntries()).toMatchObject([{ queueId: K1, upNext: 5 }]);

			// The server is heard from before the sender has sent anything.
			net.up = true;
			one.store.message({ type: 'queuesChanged' });
			await settle();
			expect(one.keptEntries()).toMatchObject([{ queueId: K1, upNext: 5 }]);

			// Once sent, the server's entry and state replace the device's, with no message needed.
			const asked = () => server.requests.filter((r) => r.path === `/me/queues/${K1}`).length;
			const before = asked();
			one.sender.wake();
			await one.sender.idle();
			await settle();
			await settle();
			expect(one.keptEntries()).toMatchObject([{ queueId: K1, upNext: 5 }]);
			expect(one.kept()).toEqual([K1]);
			expect(asked()).toBe(before + 1);
		});

		it('forgets, offline too, a kept queue inactive for longer than kept queues are kept', async () => {
			server.kept.set(K1, { queue: adjustedAlbum(), position: null });
			server.kept.set(K2, { queue: adjustedAlbum(), position: null });
			(await loaded()).close();
			const port = env.open('7');
			const [old] = (await port.all<KeptRecord>('kept')).filter((r) => r.key === K1);
			const lastActive = new Date(Date.now() - KEPT_MS - 60_000).toISOString();
			await port.put('kept', { ...old, entry: { ...old.entry, lastActive } } as Stored);

			const again = instance();
			await again.store.resume();
			expect(again.kept()).toEqual([K2]);
			expect((await port.all('kept')).map((r) => r.key)).toEqual([K2]);
		});

		it("takes the server's list in place of its own, but keeps a queue parked by a start it has not sent", async () => {
			const at = (s: number) => new Date(server.keptSince + s * 1000).toISOString();
			server.kept.set(K1, { queue: adjustedAlbum(), position: null, at: at(2) });
			server.kept.set(K2, { queue: adjustedAlbum(), position: null, at: at(1) });
			const one = await loaded();
			const first = one.keptEntries()!.find((entry) => entry.queueId === K2);
			one.store.edit(adjust);
			one.store.start(tracksStart());
			await settle();
			expect(one.kept()).toContain(QUEUE);

			// The server dropped K1, as one inactive for 30 days; the start is not sent yet.
			server.kept.delete(K1);
			net.up = true;
			one.store.message({ type: 'queuesChanged' });
			await settle();
			expect(new Set(one.kept())).toEqual(new Set([QUEUE, K2]));
			// An entry that agrees is kept as it was.
			expect(one.keptEntries()!.find((entry) => entry.queueId === K2)).toEqual(first);

			await connect(one);
			one.store.message({ type: 'queuesChanged' });
			await settle();
			expect(new Set(one.kept())).toEqual(new Set([QUEUE, K2]));
			expect(server.parked).toEqual([QUEUE]);
		});

		it('drops a queue deleted offline at once, and a read of the list before the server has it leaves it out', async () => {
			server.kept.set(K1, { queue: adjustedAlbum(), position: null });
			server.kept.set(K2, { queue: adjustedAlbum(), position: null });
			const one = await loaded();
			one.store.deleteKept(K1);
			expect(one.kept()).toEqual([K2]);
			await settle();

			net.up = true;
			one.store.message({ type: 'queuesChanged' });
			await settle();
			expect(one.kept()).toEqual([K2]);
			await connect(one);
			expect(server.kept.has(K1)).toBe(false);
			expect(one.kept()).toEqual([K2]);
			expect((await env.open('7').all('kept')).map((r) => r.key)).toEqual([K2]);
		});

		it('takes back a restore made offline that the server refuses, with the edits made on it, and says so once', async () => {
			server.kept.set(K1, { queue: adjustedAlbum(), position: null });
			const one = await loaded();
			expect(await one.store.restore(K1, true)).toMatchObject({ ok: true });
			one.store.edit(next);
			// Deleted on another device meanwhile.
			server.kept.delete(K1);

			await connect(one);
			await settle();
			expect(posts()).toEqual([`/me/queues/${K1}/restore`]);
			expect(one.refusals).toMatchObject([{ code: 'not_found', edit: null }]);
			expect(one.summary()).toMatchObject({ queueId: QUEUE, current: 'a.0' });
			expect(one.kept()).toEqual([]);
		});
	});
});
