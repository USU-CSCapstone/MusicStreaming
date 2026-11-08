import { describe, expect, it, vi } from 'vitest';
import type { RealtimeServerMessage } from '$lib/api/types';
import { CatalogClient } from './client.svelte';
import type { FromWorker, ToWorker } from './protocol';

/** A worker the test plays the catalog for. */
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

/** A client on fake workers, and a realtime connection the test can speak through. */
function setup() {
	const workers: FakeWorker[] = [];
	const client = new CatalogClient(() => {
		workers.push(new FakeWorker());
		return workers[workers.length - 1] as unknown as Worker;
	});
	let hear: (message: RealtimeServerMessage) => void = () => {};
	const realtime = {
		on: (listener: (message: RealtimeServerMessage) => void) => {
			hear = listener;
			return () => {};
		}
	};
	return { workers, client, realtime, hear: (m: object) => hear(m as RealtimeServerMessage) };
}

describe('CatalogClient', () => {
	it('starts the worker for the account and passes on only the messages that make it pull', () => {
		const { workers, client, realtime, hear } = setup();
		client.connect(realtime, '7');
		hear({ type: 'hello', deviceId: '1', serverTime: '' });
		hear({ type: 'devices', devices: [] });
		hear({ type: 'libraryChanged', libraryId: '1', seq: 5 });
		hear({ type: 'accountChanged', seq: 3 });

		expect(workers[0].sent.map((m) => m.type)).toEqual([
			'init',
			'start',
			'message',
			'message',
			'message'
		]);
		expect(workers[0].sent[0]).toMatchObject({ type: 'init', user: '7' });
	});

	it('stops the worker on disconnect', () => {
		const { workers, client, realtime } = setup();
		client.connect(realtime, '7');
		client.disconnect();
		expect(workers[0].terminated).toBe(true);
	});

	it('deletes the files with a worker started for it, which it stops when they are gone', () => {
		const { workers, client } = setup();
		client.signOut();
		expect(workers[0].sent.map((m) => m.type)).toEqual(['init', 'signOut']);
		workers[0].say({ type: 'signedOut' });
		expect(workers[0].terminated).toBe(true);
	});

	it('asks the browser to keep the storage, once', () => {
		const persist = vi.spyOn(navigator.storage, 'persist').mockResolvedValue(true);
		const { client, realtime } = setup();
		client.connect(realtime, '7');
		client.disconnect();
		client.connect(realtime, '7');
		expect(persist).toHaveBeenCalledTimes(1);
		vi.restoreAllMocks();
	});

	it('connects even while a worker started to delete the files is still alive', () => {
		const { workers, client, realtime } = setup();
		client.signOut();
		client.connect(realtime, '7');

		expect(workers[0].terminated).toBe(true);
		expect(workers[1].sent.map((m) => m.type)).toEqual(['init', 'start']);
	});
});
