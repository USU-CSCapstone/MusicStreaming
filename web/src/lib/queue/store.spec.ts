import { readFileSync } from 'node:fs';
import { beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { initSync, Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type { QueueEdit, RealtimeServerMessage, SentEdit, StartQueue } from '$lib/api/types';
import type { Refusal, Summary } from './protocol';
import { QueueStore } from './store';

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
	rowsGate: Promise<void> | null = null;
	/** Holds back the answer to an edit, once it is applied, until it is opened. */
	editGate: Promise<void> | null = null;
	/** Holds back the answer to a start until it is opened. */
	startGate: Promise<void> | null = null;
	/** What a recipe resolves to, as the tracks of the list the server holds for a recipe's start. */
	resolved5 = ['200', '201', '202', '203', '204'];
	/** The status pages of rows are answered with instead of the page. */
	rowsStatus = 0;
	/** Edits applied whose answer is lost on its way, in count. */
	loseAnswers = 0;
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
		const repeat = JSON.parse(this.queue.toJson()).repeat;
		const state = JSON.parse(Queue.started(source.key, 'lib', tracks.length, at, repeat).toJson());
		state.sources[0].list = list;
		this.queue = Queue.fromJson(JSON.stringify(state));
		this.queueId = queueId;
		this.version = 0;
		this.lists.set(list, tracks);
		return new Response(JSON.stringify({ list, length: tracks.length }));
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
			const tracks = this.lists.get(path.split('/').pop()!);
			if (!tracks) return problem(404, 'not_found');
			return new Response(JSON.stringify({ libraryId: 'lib', tracks }));
		}
		if (path.startsWith('/me/queue/rows')) {
			await this.rowsGate;
			return this.rowsStatus ? problem(this.rowsStatus, 'failed') : this.rowsPage(path);
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

let server: FakeServer;
let store: QueueStore;
let summary: Summary;
let refusals: Refusal[];
let caches: FakeCaches;

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
	// A store from an earlier test can still be retrying; only this test's store reports here.
	const created: QueueStore = new QueueStore({
		api: API,
		fetch: server.fetch as typeof fetch,
		retryMs: 0,
		storage: caches as unknown as CacheStorage,
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

	it('sends an edit once when its answer is lost', async () => {
		await hello();
		server.loseAnswers = 1;
		store.edit(next);
		await settle();

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

		expect(refusals).toEqual([{ code: 'anchor_gone', message: 'anchor_gone', edit: move }]);
		expect(summary.upNext).toBe(4);
		store.message(removal);
		expect(summary).toMatchObject({ upNext: 3, version: 1 });
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

	it('sends an add of a recipe in its turn and shows it when the server answers', async () => {
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

		expect(refusals).toMatchObject([{ code: 'superseded', edit: queueManual('m') }]);
		expect(summary).toMatchObject({ queueId: server.queueId, manual: 0 });
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

		expect(refusals).toMatchObject([{ code: 'superseded', edit: queueManual('m') }]);
		expect(summary).toMatchObject({ queueId: server.queueId, manual: 0 });
		expect(server.requests.filter((r) => r.path === '/me/queue/edits')).toEqual([]);
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
			fetch: server.fetch as typeof fetch,
			retryMs: 0,
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

		it("fetches the server's queue once it answers a recipe, and fills the rest from its list", async () => {
			await hello();
			store.start(recipeStart());
			await settle();

			expect(server.gets()).toBe(2);
			expect(server.queueGets().at(-1)!.headers).not.toHaveProperty('If-None-Match');
			expect(summary).toMatchObject({ queueId: STARTED, current: `${SOURCE}.1`, upNext: 3 });
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

		it('shows rows only the server could tell as unavailable while it cannot be reached, and waits again when it can', async () => {
			await hello();
			server.failures = [0];
			store.start(recipeStart());
			await Promise.resolve();
			await new Promise((resolve) => setTimeout(resolve, 0));

			expect(now()).toEqual(['201', '202', 'unavailable', 'unavailable']);
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

			expect(refusals).toMatchObject([
				{ code: 'superseded', edit: { type: 'next' } },
				{ code: 'failed', edit: null }
			]);
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
