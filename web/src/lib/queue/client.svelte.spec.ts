import { describe, expect, it } from 'vitest';
import type { RealtimeServerMessage } from '$lib/api/types';
import { QueueClient } from './client.svelte';
import type { FromWorker, ToWorker } from './protocol';

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
	client.connect({
		on: (listener) => {
			hear = listener;
			return () => {};
		}
	});
	return { worker, client, hear: (message: object) => hear(message as RealtimeServerMessage) };
}

describe('QueueClient', () => {
	it('tells the worker where the API is, and passes on only the queue messages and edits', () => {
		const { worker, client, hear } = setup();
		hear({ type: 'hello', deviceId: '1', serverTime: '' });
		hear({ type: 'devices', devices: [] });
		hear({ type: 'queueReplaced', queueId: 'q' });
		client.edit({ type: 'next' });

		expect(worker.sent.map((m) => m.type)).toEqual(['init', 'message', 'message', 'edit']);
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
		expect(client.counts).toEqual({ manual: 1, upNext: 4, history: 0 });
		expect(client.refusal?.code).toBe('anchor_gone');

		client.disconnect();
		expect(worker.terminated).toBe(true);
		expect(client.queueId).toBeNull();
	});
});
