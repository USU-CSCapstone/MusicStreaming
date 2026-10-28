import { readFileSync } from 'node:fs';
import { beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { initSync, Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type { QueueEdit, RealtimeServerMessage } from '$lib/api/types';
import type { Refusal, StartArgs, Summary } from './protocol';
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

/** A source of `n` tracks `<key>-0`... in library `lib`. */
const source = (key: string, n: number) => ({
	key,
	libraryId: 'lib',
	tracks: Array.from({ length: n }, (_, i) => `${key}-${i}`)
});

/** The IDs of `a`'s items at these positions. */
const ids = (key: string, ...positions: number[]) => positions.map((i) => `${key}.${i}`);

type Request = { method: string; path: string; body?: any }; // eslint-disable-line @typescript-eslint/no-explicit-any

/** A server holding one queue in the core, answering as the API does. */
class FakeServer {
	queue: Queue | null = albumQueue();
	queueId = QUEUE;
	version = 0;
	requests: Request[] = [];
	/** Statuses to answer the next requests with instead, in order; 0 is a lost connection. */
	failures: number[] = [];
	#answered = new Map<string, number>();

	/** An edit another device made: applied here, and the message every device hears. */
	apply(edit: QueueEdit, editId = crypto.randomUUID()): RealtimeServerMessage {
		this.queue!.apply(JSON.stringify(edit));
		this.version++;
		this.#answered.set(editId, this.version);
		return { type: 'queueEdit', queueId: this.queueId, version: this.version, editId, edit };
	}

	/** The `queueEdit` for the latest edit this device sent. */
	heardOf(n = -1): RealtimeServerMessage {
		const sent = this.requests.filter((r) => r.path === '/me/queue/edits');
		const { editId, edit } = sent.at(n)!.body;
		return {
			type: 'queueEdit',
			queueId: this.queueId,
			version: this.#answered.get(editId)!,
			editId,
			edit
		};
	}

	/** Answers a request as the API does, unless a failure is queued for it. */
	fetch = async (url: string | URL | Request, init?: RequestInit): Promise<Response> => {
		const path = String(url).slice(API.length);
		const method = init?.method ?? 'GET';
		const body = typeof init?.body === 'string' ? JSON.parse(init.body) : undefined;
		this.requests.push({ method, path, body });
		const failure = this.failures.shift();
		if (failure === 0) throw new TypeError('offline');
		if (failure) return problem(failure, 'failed');
		if (path === '/me/queue' && method === 'GET') {
			if (!this.queue) return problem(404, 'not_found');
			const state = JSON.parse(this.queue.snapshot());
			return json({ queueId: this.queueId, version: this.version, context: CONTEXT, state });
		}
		if (path === '/me/queue' && method === 'POST') {
			// As the server starts a queue: the source, then the Manual Queue carried over.
			const next = Queue.start(JSON.stringify(body.source), body.start);
			if (this.queue) next.carryManual(this.queue);
			this.queue = next;
			this.queueId = body.queueId;
			this.version = 0;
			return new Response(null, { status: 204 });
		}
		if (body.queueId !== this.queueId) return problem(409, 'queue_changed');
		const done = this.#answered.get(body.editId);
		if (done !== undefined) return json({ version: done });
		try {
			this.apply(body.edit, body.editId);
		} catch {
			return problem(409, 'anchor_gone');
		}
		return json({ version: this.version });
	};
}

/** A queue playing source `a` of five tracks. */
function albumQueue(): Queue {
	return Queue.start(JSON.stringify(source('a', 5)), 0);
}

/** A 200 response with a JSON body. */
const json = (body: object) => new Response(JSON.stringify(body));
/** An error response with the server's problem body. */
const problem = (status: number, code: string) =>
	new Response(JSON.stringify({ code, detail: code }), { status });

/** Lets the store's requests and retries finish. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 20));

let server: FakeServer;
let store: QueueStore;
let summary: Summary;
let refusals: Refusal[];

/** The window of `list` the store shows now. */
const window = (list: 'manual' | 'upcoming' | 'history') =>
	store.ask({ what: 'window', list, offset: 0, limit: 100 });

/** A store that has loaded the server's queue. */
async function connect() {
	store.message({ type: 'hello', deviceId: '1', serverTime: new Date().toISOString() });
	await settle();
}

const next: QueueEdit = { type: 'next' };
/** An edit that adds a one-track source `key` to the Manual Queue. */
const queueManual = (key: string): QueueEdit => ({
	type: 'add',
	source: source(key, 1),
	before: { type: 'manualEnd' }
});

/** Gives each test a fresh server and a store talking to it. */
beforeEach(() => {
	server = new FakeServer();
	refusals = [];
	store = new QueueStore({
		api: API,
		fetch: server.fetch as typeof fetch,
		retryMs: 0,
		changed: (s) => (summary = s),
		refused: (r) => refusals.push(r)
	});
});

describe('QueueStore', () => {
	it('loads the server queue when the connection says hello', async () => {
		await connect();

		expect(summary).toMatchObject({ queueId: QUEUE, version: 0, current: 'a.0', upcoming: 4 });
		expect(window('upcoming')).toEqual(ids('a', 1, 2, 3, 4));
		expect(store.ask({ what: 'track', item: 'a.2' })).toEqual({ libraryId: 'lib', trackId: 'a-2' });
		expect(store.ask({ what: 'track', item: 'zzz.1' })).toBeNull();
	});

	it('shows an edit at once, sends it, and is done with it when it comes back', async () => {
		await connect();
		store.edit(next);

		// Before anything is sent.
		expect(summary).toMatchObject({ current: 'a.1', history: 1 });
		await settle();
		const sent = server.requests.find((r) => r.path === '/me/queue/edits')!;
		expect(sent.body).toMatchObject({ queueId: QUEUE, edit: next });

		store.message(server.heardOf());
		expect(summary).toMatchObject({ current: 'a.1', history: 1, version: 1 });
		// Retired: another device's edit is not applied on top of a second `next`.
		store.message(server.apply({ type: 'remove', items: ids('a', 4) }));
		expect(window('upcoming')).toEqual(ids('a', 2, 3));
		expect(summary.current).toBe('a.1');
	});

	it('keeps ours on top of an edit from elsewhere, and agrees once ours is confirmed', async () => {
		await connect();
		store.edit(queueManual('m'));
		await settle();
		// Elsewhere, an edit the server applied before ours; the version gap means ours is v2.
		const ours = server.heardOf();
		const mine = ours.type === 'queueEdit' ? ours : null!;
		const theirs: RealtimeServerMessage = {
			type: 'queueEdit',
			queueId: QUEUE,
			version: 1,
			editId: crypto.randomUUID(),
			edit: queueManual('n')
		};
		store.message(theirs);
		expect(window('manual')).toEqual(['n.0', 'm.0']);

		store.message({ ...mine, version: 2 });
		expect(window('manual')).toEqual(['n.0', 'm.0']);
		expect(summary).toMatchObject({ version: 2, manual: 2 });
	});

	it('fetches the queue again after a gap in versions', async () => {
		await connect();
		const gets = () => server.requests.filter((r) => r.method === 'GET').length;
		server.apply(next);
		server.apply(next);
		store.message(server.apply(next));
		await settle();

		expect(gets()).toBe(2);
		expect(summary).toMatchObject({ version: 3, current: 'a.3' });
	});

	it('fetches the queue when another one replaces it, but not for its own', async () => {
		await connect();
		const gets = () => server.requests.filter((r) => r.method === 'GET').length;
		store.message({ type: 'queueReplaced', queueId: QUEUE });
		await settle();
		expect(gets()).toBe(1);

		server.queueId = '22222222-2222-4222-8222-222222222222';
		server.queue = Queue.start(JSON.stringify(source('b', 3)), 1);
		store.message({ type: 'queueReplaced', queueId: server.queueId });
		await settle();
		expect(gets()).toBe(2);
		expect(summary).toMatchObject({ queueId: server.queueId, current: 'b.1' });
	});

	it('reverts an edit whose anchor is gone, and says so', async () => {
		await connect();
		const move: QueueEdit = {
			type: 'move',
			items: ids('a', 1),
			before: { type: 'before', item: 'a.3' }
		};
		server.queue!.apply(JSON.stringify({ type: 'remove', items: ids('a', 3) }));
		server.version++;
		store.edit(move);
		expect(window('upcoming')).toEqual(ids('a', 2, 1, 3, 4));
		await settle();

		expect(window('upcoming')).toEqual(ids('a', 1, 2, 3, 4));
		expect(refusals).toEqual([{ code: 'anchor_gone', message: 'anchor_gone', edit: move }]);
	});

	it('fetches the queue when the edit is for one that was replaced', async () => {
		await connect();
		server.queueId = '22222222-2222-4222-8222-222222222222';
		server.queue = Queue.start(JSON.stringify(source('b', 3)), 0);
		store.edit(next);
		await settle();

		expect(summary).toMatchObject({ queueId: server.queueId, current: 'b.0' });
		expect(refusals).toEqual([]);
	});

	it('refuses at once an edit the queue itself refuses', async () => {
		await connect();
		const edit: QueueEdit = {
			type: 'move',
			items: ids('a', 1),
			before: { type: 'before', item: 'a.99' }
		};
		store.edit(edit);

		expect(refusals).toHaveLength(1);
		expect(refusals[0]).toMatchObject({ code: 'invalid', edit });
		expect(server.requests.filter((r) => r.method === 'POST')).toEqual([]);
	});

	it('sends an edit again, with the same ID, when the connection fails', async () => {
		await connect();
		server.failures = [0, 503];
		store.edit(next);
		await settle();

		const sends = server.requests.filter((r) => r.path === '/me/queue/edits');
		expect(sends).toHaveLength(3);
		expect(new Set(sends.map((r) => r.body.editId)).size).toBe(1);
	});

	it('does not apply an edit twice when the queue it was in was fetched after the server applied it', async () => {
		await connect();
		store.edit(next);
		await settle();
		// The `queueEdit` was lost; a reconnect fetches a queue that already has the edit.
		store.message({ type: 'hello', deviceId: '1', serverTime: new Date().toISOString() });
		await settle();

		expect(summary).toMatchObject({ current: 'a.1', history: 1, version: 1 });
	});

	describe('start', () => {
		const args = (): StartArgs => ({
			context: CONTEXT,
			libraryId: 'lib',
			tracks: ['t-0', 't-1', 't-2'],
			start: 1,
			shuffleSeed: 7
		});

		it('starts at once and tells the server, with IDs of its own', async () => {
			store.start(args());

			expect(summary).toMatchObject({ current: expect.stringMatching(/\.1$/) as string });
			await settle();
			const sent = server.requests[0];
			expect(sent).toMatchObject({ method: 'POST', path: '/me/queue' });
			expect(sent.body).toMatchObject({
				context: CONTEXT,
				start: 1,
				shuffle: { seed: 7 },
				source: { libraryId: 'lib', tracks: ['t-0', 't-1', 't-2'] }
			});
			expect(sent.body.queueId).toMatch(/^[0-9a-f-]{36}$/);
			expect(sent.body.source.key).toMatch(/^[0-9a-f-]{36}$/);
			expect(summary.queueId).toBe(sent.body.queueId);
			// The server answers `queueReplaced` for it, which needs no fetch.
			store.message({ type: 'queueReplaced', queueId: sent.body.queueId });
			await settle();
			expect(server.requests).toHaveLength(1);
		});

		it('carries the Manual Queue over as the core does', async () => {
			await connect();
			store.edit(queueManual('m'));
			store.edit({ type: 'add', source: source('n', 2), before: { type: 'manualEnd' } });
			store.start({ ...args(), shuffleSeed: undefined });

			const expected = Queue.start(JSON.stringify({ ...source('t', 3), key: 'k' }), 1);
			const old = albumQueue();
			old.apply(JSON.stringify(queueManual('m')));
			old.apply(
				JSON.stringify({ type: 'add', source: source('n', 2), before: { type: 'manualEnd' } })
			);
			expected.carryManual(old);

			const tracks = (list: string[], from: Queue | QueueStore) =>
				list.map((id) =>
					from instanceof QueueStore
						? from.ask({ what: 'track', item: id })
						: { libraryId: from.track(id)![0], trackId: from.track(id)![1] }
				);
			expect(window('manual')).toHaveLength(3);
			expect(tracks(window('manual') as string[], store)).toEqual(
				tracks(expected.manual(), expected)
			);
			expect(summary).toMatchObject({ manual: 3, upcoming: 1 });
			await settle();
		});
	});

	it('agrees with the server about the Manual Queue after a start that dropped unsent edits', async () => {
		await connect();
		// The first add is on its way, the second not yet sent.
		store.edit(queueManual('m'));
		store.edit(queueManual('n'));
		store.start({ context: CONTEXT, libraryId: 'lib', tracks: ['t-0', 't-1'], start: 0 });
		await settle();

		expect(refusals.map((r) => r.code)).toEqual(['superseded', 'superseded']);
		expect(window('manual')).toEqual(server.queue!.manual());
		expect(summary).toMatchObject({ queueId: server.queueId, manual: server.queue!.manualLen() });
	});

	it('is done with an edit whose answer was lost when a reconnect fetches a queue that has it', async () => {
		await connect();
		const real = server.fetch;
		let first = true;
		const lossy = async (url: string | URL | Request, init?: RequestInit) => {
			const response = await real(url, init);
			if (init?.method === 'POST' && first) {
				first = false;
				store.message({ type: 'hello', deviceId: '1', serverTime: new Date().toISOString() });
				throw new TypeError('lost');
			}
			return response;
		};
		store = new QueueStore({
			api: API,
			fetch: lossy as typeof fetch,
			retryMs: 0,
			changed: (s) => (summary = s),
			refused: (r) => refusals.push(r)
		});
		await connect();
		store.edit(next);
		await settle();

		expect(summary).toMatchObject({ current: 'a.1', history: 1, version: 1 });
		expect(refusals).toEqual([]);
	});

	it('is an empty queue when the account has none', async () => {
		server.queue = null;
		await connect();

		expect(summary).toMatchObject({ queueId: null, current: null, manual: 0, upcoming: 0 });
		expect(window('upcoming')).toEqual([]);
		store.edit(next);
		expect(refusals[0]).toMatchObject({ code: 'no_queue' });
	});
});
