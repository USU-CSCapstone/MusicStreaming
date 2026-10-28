import { describe, expect, it } from 'vitest';
import type { RealtimeServerMessage } from '$lib/api/types';
import { QueueClient } from './client.svelte';
import type { FromWorker, Summary, ToWorker } from './protocol';

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

/** A queue state the worker reports. */
const summary: Summary = {
	queueId: 'q',
	version: 2,
	revision: 5,
	current: 'a.0',
	manual: 1,
	upcoming: 4,
	history: 0
};

/** A client on a fake worker and a realtime connection the test can speak through. */
function setup() {
	const worker = new FakeWorker();
	const client = new QueueClient(() => worker as unknown as Worker);
	let hear: (message: RealtimeServerMessage) => void = () => {};
	client.connect({
		on: (listener) => {
			hear = listener;
			return () => {};
		}
	});
	return { worker, client, hear: (message: object) => hear(message as RealtimeServerMessage) };
}

describe('QueueClient', () => {
	it('tells the worker where the API is, and passes on only the queue messages', () => {
		const { worker, hear } = setup();
		hear({ type: 'hello', deviceId: '1', serverTime: '' });
		hear({ type: 'devices', devices: [] });
		hear({ type: 'queueReplaced', queueId: 'q' });

		expect(worker.sent.map((m) => m.type)).toEqual(['init', 'message', 'message']);
	});

	it('shows what the worker says the queue is, and the edits it reverted', () => {
		const { worker, client } = setup();
		worker.say({ type: 'state', summary });
		worker.say({ type: 'refused', refusal: { code: 'anchor_gone', message: 'gone' } });

		expect(client).toMatchObject({ queueId: 'q', current: 'a.0', revision: 5 });
		expect(client.counts).toEqual({ manual: 1, upcoming: 4, history: 0 });
		expect(client.refusal?.code).toBe('anchor_gone');
	});

	it('answers a window from the worker, and fails what is asked once closed', async () => {
		const { worker, client } = setup();
		const window = client.window('upcoming', 2, 3);
		expect(worker.sent.at(-1)).toMatchObject({
			type: 'ask',
			question: { what: 'window', list: 'upcoming', offset: 2, limit: 3 }
		});
		worker.say({ type: 'answer', id: 1, value: ['a.3', 'a.4'] });
		expect(await window).toEqual(['a.3', 'a.4']);

		const pending = client.track('a.3');
		client.disconnect();
		await expect(pending).rejects.toThrow('closed');
		await expect(client.track('a.3')).rejects.toThrow('closed');
		expect(worker.terminated).toBe(true);
		expect(client.queueId).toBeNull();
	});
});
