import { describe, expect, it, vi } from 'vitest';
import type { RealtimeServerMessage } from '$lib/api/types';
import { CatalogClient } from './client.svelte';
import type { FromWorker, ToWorker } from './protocol';
import { CatalogRouter } from './router';

/** A worker the test plays the catalog for; it answers a query with the name of its tab. */
class FakeWorker {
	sent: ToWorker[] = [];
	terminated = false;
	onmessage: ((event: MessageEvent<FromWorker>) => void) | null = null;
	onerror: (() => void) | null = null;

	constructor(
		private tab: string,
		private silent: () => boolean,
		private flags: { busy: boolean; stalled: boolean }
	) {}

	/** Records what the client sent, and answers as the real worker does. */
	postMessage(message: ToWorker) {
		this.sent.push(message);
		if (message.type === 'start') {
			if (!this.flags.stalled) this.say({ type: 'started' });
		} else if (message.type === 'busy')
			this.say({ type: 'answer', id: message.id, result: this.flags.busy ? '1' : '' });
		else if (message.type === 'pause') this.say({ type: 'paused' });
		else if (message.type === 'signOut') this.say({ type: 'answer', id: message.id, result: '' });
		else if (message.type === 'query' && !this.silent())
			this.say({ type: 'answer', id: message.id, result: JSON.stringify({ tab: this.tab }) });
	}

	/** Records that the client stopped the worker. */
	terminate() {
		this.terminated = true;
	}

	/** Delivers a message from the worker to the client, as a task of its own does. */
	say(message: FromWorker) {
		queueMicrotask(() => this.onmessage?.({ data: message } as MessageEvent<FromWorker>));
	}
}

let tabs = 0;

/** A set of tabs of one browser: they share a lock and a channel, and each has its own client and workers. */
function browser() {
	const name = `catalog-test-${++tabs}-${Math.random()}`;
	let silent = false;
	const flags = { busy: false, stalled: false };
	let hear: (message: RealtimeServerMessage) => void = () => {};
	const realtime = {
		on: (listener: (message: RealtimeServerMessage) => void) => {
			hear = listener;
			return () => {};
		}
	};
	return {
		/** Opens a tab. */
		tab(tabName: string) {
			const workers: FakeWorker[] = [];
			const client = new CatalogClient(() => {
				workers.push(new FakeWorker(tabName, () => silent, flags));
				return workers[workers.length - 1] as unknown as Worker;
			}, name);
			return { client, workers };
		},
		/** The router of a worker of the queue or any other asker, on the browser's channel. */
		router() {
			return new CatalogRouter(name, 500);
		},
		/** Makes every worker leave queries unanswered, as one that is closing does. */
		silence(on: boolean) {
			silent = on;
		},
		name,
		/** Makes workers report a snapshot downloading, or never say `started`. */
		flags,
		realtime,
		hear: (m: object) => hear(m as RealtimeServerMessage)
	};
}

/** The types of the messages the client has sent `worker`, in order. */
const types = (worker: FakeWorker) => worker.sent.map((m) => m.type);

describe('CatalogClient', () => {
	it('starts the worker for the account and passes on only the messages that make it pull', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		b.hear({ type: 'hello', deviceId: '1', serverTime: '' });
		b.hear({ type: 'devices', devices: [] });
		b.hear({ type: 'libraryChanged', libraryId: '1', seq: 5 });
		b.hear({ type: 'accountChanged', seq: 3 });

		expect(types(workers[0])).toEqual(['init', 'start', 'message', 'message', 'message']);
		expect(workers[0].sent[0]).toMatchObject({ type: 'init', user: '7' });
		client.disconnect();
	});

	it('a second tab’s query is answered by the owner', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');

		const answer = await second.client.query('1', 'listTracks', { limit: 1 });
		expect(JSON.parse(answer)).toEqual({ tab: 'first' });
		expect(second.workers).toHaveLength(0);
		// The owner asks itself without the channel, and both get the same page API.
		expect(JSON.parse(await first.client.query('1', 'listTracks'))).toEqual({ tab: 'first' });
		expect(first.workers[0].sent.filter((m) => m.type === 'query')).toHaveLength(2);
		first.client.disconnect();
		second.client.disconnect();
	});

	it('closing the owner moves ownership and pending requests are retried', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		const asker = b.tab('asker');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');

		b.silence(true);
		const pending = asker.client.query('1', 'listTracks');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('query'));
		b.silence(false);
		first.client.disconnect();

		expect(JSON.parse(await pending)).toEqual({ tab: 'second' });
		// The old owner released the pool before the lock passed on, and the new one opened after.
		expect(types(first.workers[0]).slice(-1)).toEqual(['pause']);
		expect(first.workers[0].terminated).toBe(true);
		expect(types(second.workers[0])).toEqual(['init', 'start', 'query']);
		second.client.disconnect();
	});

	it('a tab going away gives the catalog up, and takes it again when shown from the cache', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');

		await vi.waitFor(async () =>
			expect((await navigator.locks.query()).pending?.length).toBeGreaterThan(0)
		);
		// Both tabs are in this one window, so both hear it.
		dispatchEvent(new Event('pagehide'));
		await vi.waitFor(() => expect(first.workers[0].terminated).toBe(true));
		expect(types(first.workers[0]).slice(-1)).toEqual(['pause']);
		expect(second.workers).toHaveLength(0);

		dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
		await vi.waitFor(() =>
			expect([...first.workers, ...second.workers].filter((w) => !w.terminated)).toHaveLength(1)
		);
		first.client.disconnect();
		second.client.disconnect();
	});

	it('a new owner answers the request it was itself waiting on', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');

		b.silence(true);
		const pending = second.client.query('1', 'listTracks');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('query'));
		b.silence(false);
		first.client.disconnect();

		expect(JSON.parse(await pending)).toEqual({ tab: 'second' });
		second.client.disconnect();
	});

	it('an owner hidden with another tab waiting hands over, and wants the files back when shown', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);

		// No one waits: nothing changes.
		document.dispatchEvent(new Event('visibilitychange'));
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(first.workers[0].terminated).toBe(false);

		second.client.connect(b.realtime, '7');
		await vi.waitFor(async () =>
			expect((await navigator.locks.query()).pending?.length).toBeGreaterThan(0)
		);
		document.dispatchEvent(new Event('visibilitychange'));
		await vi.waitFor(() =>
			expect(types(second.workers[0] ?? ({ sent: [] } as never))).toContain('start')
		);
		expect(first.workers[0].terminated).toBe(true);
		expect(types(first.workers[0]).slice(-1)).toEqual(['pause']);

		hidden.mockReturnValue(false);
		document.dispatchEvent(new Event('visibilitychange'));
		first.client.disconnect();
		second.client.disconnect();
		vi.restoreAllMocks();
	});

	it('an owner downloading a snapshot keeps the catalog when hidden, and hands over once it is done', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(async () =>
			expect((await navigator.locks.query()).pending?.length).toBeGreaterThan(0)
		);
		const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
		b.flags.busy = true;
		document.dispatchEvent(new Event('visibilitychange'));
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('busy'));
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(first.workers[0].terminated).toBe(false);

		b.flags.busy = false;
		document.dispatchEvent(new Event('visibilitychange'));
		await vi.waitFor(() => expect(first.workers[0].terminated).toBe(true));
		hidden.mockReturnValue(false);
		first.client.disconnect();
		second.client.disconnect();
		vi.restoreAllMocks();
	});

	it('an owner whose worker fails gives the lock to the next tab', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		b.flags.stalled = true;
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		b.flags.stalled = false;

		first.workers[0].onerror?.();
		await vi.waitFor(() =>
			expect(types(second.workers[0] ?? ({ sent: [] } as never))).toContain('start')
		);
		expect(first.workers[0].terminated).toBe(true);
		first.client.disconnect();
		second.client.disconnect();
	});

	it('an owner whose worker never starts gives the lock up after 20 seconds', async () => {
		vi.useFakeTimers({ toFake: ['setTimeout'] });
		try {
			const b = browser();
			const first = b.tab('first');
			const second = b.tab('second');
			b.flags.stalled = true;
			first.client.connect(b.realtime, '7');
			await vi.waitFor(() =>
				expect(types(first.workers[0] ?? ({ sent: [] } as never))).toContain('start')
			);
			second.client.connect(b.realtime, '7');
			b.flags.stalled = false;
			await vi.advanceTimersByTimeAsync(19_000);
			expect(second.workers).toHaveLength(0);

			await vi.advanceTimersByTimeAsync(2_000);
			await vi.waitFor(() => expect(second.workers).toHaveLength(1));
			expect(first.workers[0].terminated).toBe(true);
			first.client.disconnect();
			second.client.disconnect();
		} finally {
			vi.useRealTimers();
		}
	});

	it('a tab that hears a sign-out leaves the election', async () => {
		const b = browser();
		const owner = b.tab('owner');
		const waiting = b.tab('waiting');
		const signing = b.tab('signing');
		owner.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(owner.workers[0])).toContain('start'));
		waiting.client.connect(b.realtime, '7');

		await signing.client.signOut();
		await vi.waitFor(() => expect(owner.workers[0].terminated).toBe(true));
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(waiting.workers).toHaveLength(0);
	});

	it('a request nobody answers fails after the timeout', async () => {
		const b = browser();
		await expect(
			b.router().ask({ type: 'query', library: '1', name: 'x', params: {} })
		).rejects.toThrow(/did not answer/);
	});

	it('a queue worker’s router is answered by the owner', async () => {
		const b = browser();
		const { client, workers } = b.tab('owner');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		const answer = await b
			.router()
			.ask({ type: 'query', library: '1', name: 'listTracks', params: {} });
		expect(JSON.parse(answer)).toEqual({ tab: 'owner' });
		client.disconnect();
	});

	it('deletes the files with a worker started for it, which it stops when they are gone', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		await client.signOut();
		expect(types(workers[0])).toEqual(['init', 'signOut']);
		expect(workers[0].sent[0]).toMatchObject({ user: '' });
		expect(workers[0].terminated).toBe(true);
	});

	it('signing out in one tab is done by the tab that owns the files, which then lets them go', async () => {
		const b = browser();
		const owner = b.tab('owner');
		const other = b.tab('other');
		owner.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(owner.workers[0])).toContain('start'));

		await other.client.signOut();
		expect(other.workers).toHaveLength(0);
		expect(types(owner.workers[0])).toContain('signOut');
		await vi.waitFor(() => expect(owner.workers[0].terminated).toBe(true));
	});

	it('asks the browser to keep the storage, once', async () => {
		const persist = vi.spyOn(navigator.storage, 'persist').mockResolvedValue(true);
		const b = browser();
		const { client } = b.tab('a');
		client.connect(b.realtime, '7');
		client.disconnect();
		client.connect(b.realtime, '7');
		expect(persist).toHaveBeenCalledTimes(1);
		client.disconnect();
		vi.restoreAllMocks();
	});

	it('signs out from a tab that owned the files, once it has let them go', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		client.disconnect();
		await client.signOut();

		expect(workers[0].terminated).toBe(true);
		expect(types(workers[1])).toEqual(['init', 'signOut']);
	});

	it('a sign-out under way finishes before the tab signs in again, and does not wipe the new session', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		const out = client.signOut();
		client.connect(b.realtime, '8');
		// Shown again while the sign-out is still running: no second way into the election.
		document.dispatchEvent(new Event('visibilitychange'));
		dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
		await out;

		await vi.waitFor(() => expect(types(workers[1])).toContain('start'));
		expect(types(workers[0])).toEqual(['init', 'signOut']);
		expect(workers[0].terminated).toBe(true);
		expect(workers[1].sent[0]).toMatchObject({ user: '8' });
		expect(types(workers[1])).not.toContain('signOut');
		expect(workers[1].terminated).toBe(false);
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(workers).toHaveLength(2);
		expect((await navigator.locks.query()).held?.filter((l) => l.name === b.name)).toHaveLength(1);
		client.disconnect();
	});
});
