import { readFileSync } from 'node:fs';
import { beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { initSync, Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type { QueueEdit, RealtimeServerMessage, SentEdit } from '$lib/api/types';
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
		return { ...edit, source: { key, libraryId, list: `h-${key}`, length } };
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

	/** Starts the server again: a new run, and the queue as a crash left it. */
	restart(queue: Queue, version: number) {
		this.run = 'r2';
		this.queue = queue;
		this.version = version;
		this.#answered.clear();
	}

	/** How many times the queue was asked for, and how many of those answered with the whole of it. */
	gets() {
		return this.requests.filter((r) => r.method === 'GET').length;
	}

	/** Answers a request as the API does, unless a failure is queued for it. */
	fetch = async (url: string | URL | Request, init?: RequestInit): Promise<Response> => {
		const path = String(url).slice(API.length);
		const method = init?.method ?? 'GET';
		const body = typeof init?.body === 'string' ? JSON.parse(init.body) : undefined;
		const headers = init?.headers as Record<string, string> | undefined;
		this.requests.push({ method, path, body, headers });
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
		return new Response(JSON.stringify({ version }));
	};
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
	store = new QueueStore({
		api: API,
		fetch: server.fetch as typeof fetch,
		retryMs: 0,
		changed: (s) => (summary = s),
		refused: (r) => refusals.push(r)
	});
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
		expect(server.requests[1].headers?.['If-None-Match']).toBe(`"r1.${QUEUE}.0"`);
		expect(summary.revision).toBe(revision);

		// An edit this device missed, as after a dropped connection.
		server.apply(next);
		await hello();
		expect(server.gets()).toBe(3);
		expect(server.requests[2].headers?.['If-None-Match']).toBe(`"r1.${QUEUE}.0"`);
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
