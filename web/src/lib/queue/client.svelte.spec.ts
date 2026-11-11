import { describe, expect, it, onTestFinished, vi } from 'vitest';
import type { RealtimeServerMessage } from '$lib/api/types';
import { QueueClient } from './client.svelte';
import type { FromWorker, StartRequest, ToWorker } from './protocol';

/** A worker the test plays the queue for. */
class FakeWorker {
	sent: ToWorker[] = [];
	terminated = false;
	onmessage: ((event: MessageEvent<FromWorker>) => void) | null = null;

	/** Records what the client sent. */
	postMessage(message: ToWorker) {
		this.sent.push(message);
	}

	/** Records that the client stopped the worker. */
	terminate() {
		this.terminated = true;
	}

	/** Delivers a message from the worker to the client. */
	say(message: FromWorker) {
		this.onmessage?.({ data: message } as MessageEvent<FromWorker>);
	}
}

/** A client on a fake worker and a realtime connection the test can speak through. */
function setup() {
	const worker = new FakeWorker();
	const client = new QueueClient(() => worker as unknown as Worker);
	let hear: (message: RealtimeServerMessage) => void = () => {};
	client.connect(
		{
			on: (listener) => {
				hear = listener;
				return () => {};
			}
		},
		'7'
	);
	return { worker, client, hear: (message: object) => hear(message as RealtimeServerMessage) };
}

describe('QueueClient', () => {
	it('tells the worker of each hide and unhide the marks show, until it disconnects', () => {
		const worker = new FakeWorker();
		const client = new QueueClient(() => worker as unknown as Worker);
		let tell: (library: string, track: string, hidden: boolean) => void = () => {};
		let stopped = false;
		client.connect({ on: () => () => {} }, '7', {
			onHidden: (listener) => {
				tell = listener;
				return () => (stopped = true);
			}
		});
		tell('1', '5', true);
		expect(worker.sent.at(-1)).toEqual({
			type: 'hidden',
			libraryId: '1',
			trackId: '5',
			hidden: true
		});
		client.disconnect();
		expect(stopped).toBe(true);
	});

	it('tells the worker where the API is and whose queue it is, and passes on only the queue messages and edits', () => {
		const { worker, client, hear } = setup();
		hear({ type: 'hello', deviceId: '1', serverTime: '' });
		hear({ type: 'devices', devices: [] });
		hear({ type: 'queueReplaced', queueId: 'q' });
		client.edit({ type: 'next' });

		expect(worker.sent.map((m) => m.type)).toEqual(['init', 'message', 'message', 'edit']);
		expect(worker.sent[0]).toMatchObject({ type: 'init', user: '7' });
	});

	it('shows what the worker says the queue is, and the edit that did not stay', () => {
		const { worker, client } = setup();
		worker.say({
			type: 'state',
			summary: {
				queueId: 'q',
				context: { type: 'album', id: '1' },
				version: 2,
				revision: 5,
				current: 'a.0',
				playingManual: true,
				held: 'a.0',
				manual: 1,
				upNext: 4,
				history: 0,
				repeat: 'all',
				shuffled: true
			}
		});
		const edit = { type: 'next' } as const;
		worker.say({ type: 'refused', refusal: { code: 'anchor_gone', message: 'gone', edit } });

		expect(client).toMatchObject({ queueId: 'q', current: 'a.0', revision: 5, repeat: 'all' });
		expect(client.playingManual).toBe(true);
		expect(client.counts).toEqual({ manual: 1, upNext: 4, history: 0 });
		expect(client.refusal?.code).toBe('anchor_gone');

		client.disconnect();
		expect(worker.terminated).toBe(true);
		expect(client.queueId).toBeNull();
	});

	it('asks the worker for a window of rows and gets the answer meant for it', async () => {
		const { worker, client } = setup();
		const first = client.rows('upNext', 0, 10);
		const second = client.rows('history', 0, 5);
		const [one, two] = worker.sent.slice(1) as Extract<ToWorker, { type: 'rows' }>[];
		const window = (section: 'upNext' | 'history') => ({
			section,
			offset: 0,
			total: 0,
			revision: 1,
			rows: []
		});
		worker.say({ type: 'rows', id: two.id, window: window('history') });
		worker.say({ type: 'rows', id: one.id, window: window('upNext') });

		expect(one).toMatchObject({ section: 'upNext', offset: 0, limit: 10 });
		expect((await first).section).toBe('upNext');
		expect((await second).section).toBe('history');
	});

	it('settles a start with what the worker says of it', async () => {
		const { worker, client } = setup();
		const request: StartRequest = {
			queueId: 'q',
			context: { type: 'library' } as StartRequest['context'],
			source: { key: 'k', libraryId: '1', tracks: ['5'] },
			start: 0
		};
		const taken = client.start(request);
		const refused = client.start({ ...request, start: 3 });
		const [one, two] = worker.sent.slice(1) as Extract<ToWorker, { type: 'start' }>[];
		worker.say({ type: 'started', id: two.id, ok: false });
		worker.say({ type: 'started', id: one.id, ok: true });

		expect(one).toMatchObject({ type: 'start', request });
		expect(await taken).toBe(true);
		expect(await refused).toBe(false);
	});

	it('tells the worker whether the app counts itself offline with each edit and start', () => {
		const onLine = vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false);
		onTestFinished(() => onLine.mockRestore());
		const { worker, client } = setup();
		client.edit({ type: 'next' });
		client.start({
			queueId: 'q',
			context: { type: 'library' } as StartRequest['context'],
			source: { key: 'k', libraryId: '1', tracks: ['5'] },
			start: 0
		});
		const [edit, start] = worker.sent.slice(1) as Extract<ToWorker, { type: 'edit' | 'start' }>[];
		expect(edit).toMatchObject({ type: 'edit', offline: true });
		expect(start).toMatchObject({ type: 'start', request: { offline: true } });
	});

	it('settles each restore with the worker answer to it', async () => {
		const { worker, client } = setup();
		const first = client.restore('q1');
		const second = client.restore('q2');
		const [one, two] = worker.sent.slice(1) as Extract<ToWorker, { type: 'restore' }>[];
		worker.say({
			type: 'restored',
			id: two.id,
			result: { ok: false, code: 'queue_active', message: '' }
		});
		worker.say({ type: 'restored', id: one.id, result: { ok: true, position: null } });

		expect(one).toMatchObject({ type: 'restore', queueId: 'q1' });
		expect(await first).toEqual({ ok: true, position: null });
		expect(await second).toMatchObject({ ok: false, code: 'queue_active' });
	});
});
