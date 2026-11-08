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
		private flags: {
			busy: boolean;
			stalled: boolean;
			slow: boolean;
			mute: boolean;
			onDevice: string;
		}
	) {}

	/** Records what the client sent, and answers as the real worker does. */
	postMessage(message: ToWorker) {
		this.sent.push(message);
		if (message.type === 'start') {
			if (this.flags.slow) setTimeout(() => this.say({ type: 'started' }), 300);
			else if (!this.flags.stalled) this.say({ type: 'started' });
		} else if (message.type === 'busy')
			this.say({ type: 'answer', id: message.id, result: this.flags.busy ? '1' : '' });
		else if (message.type === 'onDevice' && !this.silent())
			this.say({ type: 'answer', id: message.id, result: this.flags.onDevice });
		else if (message.type === 'pause') this.say({ type: 'paused' });
		else if (message.type === 'signOut' && !this.flags.mute)
			this.say({ type: 'answer', id: message.id, result: '' });
		else if (message.type === 'query' && !this.silent())
			this.say({
				type: 'answer',
				id: message.id,
				result: JSON.stringify({ tab: this.tab })
			});
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
	const flags = { busy: false, stalled: false, slow: false, mute: false, onDevice: '1' };
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
		/** Makes workers report a snapshot downloading, never say `started`, say it late, or never answer a sign-out. */
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

	it('a library is not on the device with no owner, and the owner’s answer from any tab', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		expect(await second.client.onDevice('1')).toBe(false);

		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		expect(await second.client.onDevice('1')).toBe(true);
		b.flags.onDevice = '';
		expect(await first.client.onDevice('1')).toBe(false);
		first.client.disconnect();
		second.client.disconnect();
	});

	it('a library’s place falls back at once with no owner, and after the state wait with a silent one', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		const since = performance.now();
		expect(await second.client.onDevice('1')).toBe(false);
		expect(performance.now() - since).toBeLessThan(500);

		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		b.silence(true);
		const asked = performance.now();
		expect(await second.client.onDevice('1')).toBe(false);
		// The state wait, not the router's own (a second tab's router waits 10 s by default).
		expect(performance.now() - asked).toBeGreaterThan(1000);
		expect(performance.now() - asked).toBeLessThan(5000);
		first.client.disconnect();
	});

	it('a request’s own timeout replaces the router’s', async () => {
		const b = browser();
		const router = new CatalogRouter(b.name, 5000);
		const since = performance.now();
		await expect(router.ask({ type: 'onDevice', library: '1' }, 100)).rejects.toThrow();
		expect(performance.now() - since).toBeLessThan(1000);
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
		expect(JSON.parse(await first.client.query('1', 'listTracks'))).toEqual({
			tab: 'first'
		});
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

	it('sign-out empties the image cache', async () => {
		const images = await caches.open('images');
		await images.put('/api/v1/libraries/1/images/9?size=96', new Response('jpeg'));
		const b = browser();
		await b.tab('a').client.signOut();
		expect(await caches.has('images')).toBe(false);
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

	it('an owner started for another account than the session’s answers no one and gives the catalog up', async () => {
		const session = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ id: '8' }));
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '8');

		// Refused by the stale owner, or already answered by the next one: never from the old account.
		await second.client.query('1', 'listTracks').then(
			(answer) => expect(JSON.parse(answer)).toEqual({ tab: 'second' }),
			(error) => expect(error.message).toMatch(/another account/)
		);
		await vi.waitFor(() => expect(first.workers[0].terminated).toBe(true));
		await vi.waitFor(() =>
			expect(types(second.workers[0] ?? ({ sent: [] } as never))).toContain('start')
		);
		expect(JSON.parse(await second.client.query('1', 'listTracks'))).toEqual({ tab: 'second' });
		expect(types(first.workers[0])).not.toContain('query');
		second.client.disconnect();
		session.mockRestore();
	});

	it('an owner whose session has ended gives the catalog up when another account asks', async () => {
		const session = vi
			.spyOn(globalThis, 'fetch')
			.mockResolvedValue(new Response(null, { status: 401 }));
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '8');

		await second.client.query('1', 'listTracks').catch(() => {});
		await vi.waitFor(() => expect(first.workers[0].terminated).toBe(true));
		second.client.disconnect();
		session.mockRestore();
	});

	it('a stale tab’s query is refused and the owner of the session’s account stays owner', async () => {
		const session = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ id: '8' }));
		const b = browser();
		const owner = b.tab('owner');
		const stale = b.tab('stale');
		owner.client.connect(b.realtime, '8');
		await vi.waitFor(() => expect(types(owner.workers[0])).toContain('start'));
		stale.client.connect(b.realtime, '7');

		await expect(stale.client.query('1', 'listTracks')).rejects.toThrow(/another account/);
		await vi.waitFor(() => expect(session).toHaveBeenCalled());
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(owner.workers[0].terminated).toBe(false);
		expect(JSON.parse(await owner.client.query('1', 'listTracks'))).toEqual({ tab: 'owner' });
		owner.client.disconnect();
		stale.client.disconnect();
		session.mockRestore();
	});

	it('a sign-out the local worker never answers is given up on after the router’s timeout', async () => {
		vi.useFakeTimers({ toFake: ['setTimeout'] });
		try {
			const b = browser();
			b.flags.mute = true;
			const { client, workers } = b.tab('a');
			let done = false;
			const out = client.signOut().then(() => (done = true));
			await vi.waitFor(() => expect(workers).toHaveLength(1));
			await vi.advanceTimersByTimeAsync(9_000);
			expect(done).toBe(false);
			await vi.advanceTimersByTimeAsync(2_000);
			await out;
			expect(workers[0].terminated).toBe(true);
		} finally {
			vi.useRealTimers();
		}
	});

	it('signing out while another tab waits for the lock does not wait for the router’s timeout', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		b.flags.slow = true;
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(async () =>
			expect((await navigator.locks.query()).pending?.length).toBeGreaterThan(0)
		);

		first.client.disconnect();
		const began = Date.now();
		await first.client.signOut();
		expect(Date.now() - began).toBeLessThan(5_000);
		second.client.disconnect();
	}, 30_000);

	it('without Web Locks nothing waits: queries fail at once and signing out resolves', async () => {
		const locks = Object.getOwnPropertyDescriptor(Navigator.prototype, 'locks')!;
		Object.defineProperty(Navigator.prototype, 'locks', {
			configurable: true,
			value: undefined
		});
		try {
			const b = browser();
			const { client, workers } = b.tab('a');
			client.connect(b.realtime, '7');
			await expect(client.query('1', 'listTracks')).rejects.toThrow(/not available/);
			await client.signOut();
			expect(workers).toHaveLength(0);
			client.disconnect();
		} finally {
			Object.defineProperty(Navigator.prototype, 'locks', locks);
		}
	});
});
