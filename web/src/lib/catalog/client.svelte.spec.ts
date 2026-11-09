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

/** How many requests for the lock `name` wait. */
const waiting = async (name: string) =>
	((await navigator.locks.query()).pending ?? []).filter((lock) => lock.name === name).length;

/** A set of tabs of one browser: they share a lock and a channel, and each has its own client and workers. */
function browser() {
	const name = `catalog-test-${++tabs}-${Math.random()}`;
	let silent = false;
	const flags = { busy: false, stalled: false, slow: false, mute: false, onDevice: '1' };
	let hear: (message: RealtimeServerMessage) => void = () => {};
	/** A realtime connection whose messages the test sends through `hear`. */
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
			// Whether the tab is out of sight; `show` and `hide` change it and tell the tab.
			const view = { hidden: false };
			const client = new CatalogClient(
				() => {
					workers.push(new FakeWorker(tabName, () => silent, flags));
					return workers[workers.length - 1] as unknown as Worker;
				},
				name,
				() => view.hidden
			);
			const change = (hidden: boolean) => {
				view.hidden = hidden;
				document.dispatchEvent(new Event('visibilitychange'));
			};
			return { client, workers, hide: () => change(true), show: () => change(false) };
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

	it('tells the owner’s worker whether the device is working offline, as it starts and when it changes', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.setWorkingOffline(true);
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		expect(types(workers[0]).slice(0, 3)).toEqual(['init', 'offline', 'start']);
		client.setWorkingOffline(false);
		expect(workers[0].sent.at(-1)).toEqual({ type: 'offline', value: false });
		client.disconnect();
	});

	it('a tab that joined the election early answers a read once its owner is up, and connect keeps that owner', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.setWorkingOffline(true);
		client.preconnect('7');
		// Offline and asked at once, before the election has the lock: it waits rather than saying no.
		expect(await client.onDevice('1')).toBe(true);
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		expect(workers).toHaveLength(1);
		b.hear({ type: 'hello', deviceId: '1', serverTime: '' });
		expect(types(workers[0])).toContain('message');
		// The same account keeps its owner: nothing was paused or stopped.
		expect(types(workers[0])).not.toContain('pause');
		expect(workers[0].terminated).toBe(false);
		client.disconnect();
	});

	it('another account never reads the files of the account that preconnected: its owner is replaced', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.preconnect('7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		client.connect(b.realtime, '8');
		await vi.waitFor(() => expect(workers).toHaveLength(2));
		expect(types(workers[0])).toContain('pause');
		expect(workers[0].terminated).toBe(true);
		expect(workers[1].sent[0]).toMatchObject({ type: 'init', user: '8' });
		client.disconnect();
	});

	it('a tab whose worker failed answers none at once, not after waiting for an owner', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		workers[0].onerror?.();
		// The lock is given back once the failed worker is stopped.
		await vi.waitFor(async () =>
			expect((await navigator.locks.query()).held?.some((lock) => lock.name === b.name)).toBe(false)
		);
		const began = Date.now();
		expect(await client.onDevice('1')).toBe(false);
		expect(Date.now() - began).toBeLessThan(1000);
	});

	it('a tab that joined early can still sign out', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.preconnect('7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		await client.signOut();
		expect(types(workers.at(-1)!)).toContain('signOut');
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

	it('an owner hidden with a waiting tab in view hands over, and wants the files back when shown', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));

		// No one waits: nothing changes.
		first.hide();
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(first.workers[0].terminated).toBe(false);
		first.show();

		second.client.connect(b.realtime, '7');
		await vi.waitFor(async () =>
			expect((await navigator.locks.query()).pending?.length).toBeGreaterThan(0)
		);
		first.hide();
		await vi.waitFor(() =>
			expect(types(second.workers[0] ?? ({ sent: [] } as never))).toContain('start')
		);
		expect(first.workers[0].terminated).toBe(true);
		expect(types(first.workers[0]).slice(-1)).toEqual(['pause']);

		first.show();
		first.client.disconnect();
		second.client.disconnect();
	});

	it('an owner hidden keeps the catalog while the tabs waiting for it are hidden too, and hands over when one is shown', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(async () => expect(await waiting(b.name)).toBe(1));
		// A hidden waiting tab leaves the queue for the lock.
		second.hide();
		await vi.waitFor(async () => expect(await waiting(b.name)).toBe(0));

		first.hide();
		await new Promise((resolve) => setTimeout(resolve, 150));
		expect(first.workers[0].terminated).toBe(false);
		expect(second.workers).toHaveLength(0);

		second.show();
		await vi.waitFor(() =>
			expect(types(second.workers[0] ?? ({ sent: [] } as never))).toContain('start')
		);
		expect(first.workers[0].terminated).toBe(true);
		first.client.disconnect();
		second.client.disconnect();
	});

	it('the catalog goes to a waiting tab in view, never to a hidden tab that joined before it', async () => {
		const b = browser();
		const a = b.tab('a');
		const hiddenTab = b.tab('b');
		const visibleTab = b.tab('c');
		a.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(a.workers[0])).toContain('start'));
		hiddenTab.hide();
		hiddenTab.client.connect(b.realtime, '7');
		visibleTab.client.connect(b.realtime, '7');
		await vi.waitFor(async () => expect(await waiting(b.name)).toBe(1));

		a.hide();
		await vi.waitFor(() =>
			expect(types(visibleTab.workers[0] ?? ({ sent: [] } as never))).toContain('start')
		);
		expect(hiddenTab.workers).toHaveLength(0);
		a.client.disconnect();
		hiddenTab.client.disconnect();
		visibleTab.client.disconnect();
	});

	it('a tab out of sight owns the catalog when no one holds it, and is not queued when someone does', async () => {
		const b = browser();
		const alone = b.tab('alone');
		alone.hide();
		alone.client.connect(b.realtime, '7');
		await vi.waitFor(() =>
			expect(types(alone.workers[0] ?? ({ sent: [] } as never))).toContain('start')
		);

		const other = b.tab('other');
		other.hide();
		other.client.connect(b.realtime, '7');
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(await waiting(b.name)).toBe(0);
		alone.client.disconnect();
		other.client.disconnect();
	});

	it('says what each tab has to say: an owner hidden says so, and a tab in view that waits asks', async () => {
		const b = browser();
		const heard: string[] = [];
		const probe = new BroadcastChannel(b.name);
		probe.onmessage = ({ data }) => data.type === 'turn' && heard.push(data.kind);
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		await vi.waitFor(() => expect(heard).toEqual(['want']));
		heard.length = 0;
		first.hide();
		await vi.waitFor(() => expect(heard).toEqual(['hidden']));
		first.show();

		// A tab that joins in view asks at once, and one shown again after leaving the queue asks too.
		second.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(heard).toEqual(['hidden', 'want']));
		second.hide();
		await vi.waitFor(async () => expect(await waiting(b.name)).toBe(0));
		second.show();
		await vi.waitFor(() => expect(heard).toEqual(['hidden', 'want', 'want']));
		probe.close();
		first.client.disconnect();
		second.client.disconnect();
	});

	it('a hidden owner that is asked for the catalog gives it up, and a visible one does not', async () => {
		const b = browser();
		const probe = new BroadcastChannel(b.name);
		const first = b.tab('first');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		probe.postMessage({ type: 'turn', kind: 'want' });
		await new Promise((resolve) => setTimeout(resolve, 150));
		expect(first.workers[0].terminated).toBe(false);

		first.hide();
		probe.postMessage({ type: 'turn', kind: 'want' });
		await vi.waitFor(() => expect(first.workers[0].terminated).toBe(true));
		probe.close();
		first.client.disconnect();
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
		b.flags.busy = true;
		first.hide();
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('busy'));
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(first.workers[0].terminated).toBe(false);

		b.flags.busy = false;
		second.show();
		await vi.waitFor(() => expect(first.workers[0].terminated).toBe(true));
		first.client.disconnect();
		second.client.disconnect();
	});

	it('an owner whose page is frozen stops its worker and releases the lock in microtasks, with no wait for a task', async () => {
		const b = browser();
		const first = b.tab('first');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));

		document.dispatchEvent(new Event('freeze'));
		// A frozen page runs no tasks: nothing but microtasks may stand between this and the end of its ownership.
		for (let i = 0; i < 20; i++) await Promise.resolve();
		expect(first.workers[0].terminated).toBe(true);
		expect(types(first.workers[0])).not.toContain('pause');
		await vi.waitFor(async () =>
			expect((await navigator.locks.query()).held?.some((lock) => lock.name === b.name)).toBe(false)
		);

		document.dispatchEvent(new Event('resume'));
		await vi.waitFor(() => expect(first.workers).toHaveLength(2));
		await vi.waitFor(() => expect(types(first.workers[1])).toContain('start'));

		// Owning again, a normal hand-over does the pause round trip: nothing is still in a hurry.
		const second = b.tab('second');
		second.client.connect(b.realtime, '7');
		await vi.waitFor(async () => expect(await waiting(b.name)).toBe(1));
		first.hide();
		await vi.waitFor(() => expect(first.workers[1].terminated).toBe(true));
		expect(types(first.workers[1])).toContain('pause');
		first.client.disconnect();
		second.client.disconnect();
	});

	it('a tab that started out of view and was shown before it connected joins the queue when it connects', async () => {
		const b = browser();
		const owner = b.tab('owner');
		const late = b.tab('late');
		owner.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(owner.workers[0])).toContain('start'));
		late.hide();
		late.client.preconnect('7');
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(await waiting(b.name)).toBe(0);
		late.show();

		// No visibility event follows: connecting alone has to bring it into the queue.
		late.client.connect(b.realtime, '7');
		await vi.waitFor(async () => expect(await waiting(b.name)).toBe(1));
		owner.client.disconnect();
		late.client.disconnect();
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

	it('sign-out empties the image cache and the queue lists', async () => {
		const images = await caches.open('images');
		await images.put('/api/v1/libraries/1/images/9?size=96', new Response('jpeg'));
		const lists = await caches.open('jewelcase-queue-lists');
		await lists.put('/api/v1/me/queue/lists/abc', new Response('{}'));
		const b = browser();
		await b.tab('a').client.signOut();
		expect(await caches.has('images')).toBe(false);
		expect(await caches.has('jewelcase-queue-lists')).toBe(false);
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

	it('online, a read does not wait for this tab’s own worker to start', async () => {
		const b = browser();
		b.flags.slow = true;
		const { client, workers } = b.tab('a');
		client.preconnect('7');
		await vi.waitFor(() => expect(types(workers[0] ?? ({ sent: [] } as never))).toContain('start'));
		const began = performance.now();
		expect(await client.onDevice('1')).toBe(false);
		expect(performance.now() - began).toBeLessThan(150);
		client.disconnect();
	});

	it('online, a read goes to the server without waiting for an owner that did not answer, until one is serving', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		b.silence(true);
		expect(await second.client.onDevice('1')).toBe(false);
		const again = performance.now();
		expect(await second.client.onDevice('1')).toBe(false);
		expect(performance.now() - again).toBeLessThan(150);

		// Once this tab serves the catalog itself, its reads go to the device again at once.
		b.silence(false);
		second.client.connect(b.realtime, '7');
		first.client.disconnect();
		await vi.waitFor(async () => expect(await second.client.onDevice('1')).toBe(true));
		second.client.disconnect();
	});

	it('an owner that has to check who the session is makes no request while working offline', async () => {
		const session = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ id: '8' }));
		const b = browser();
		const owner = b.tab('owner');
		const stale = b.tab('stale');
		owner.client.connect(b.realtime, '8');
		owner.client.setWorkingOffline(true);
		await vi.waitFor(() => expect(types(owner.workers[0])).toContain('start'));
		stale.client.connect(b.realtime, '7');

		await expect(stale.client.query('1', 'listTracks')).rejects.toThrow(/another account/);
		await new Promise((resolve) => setTimeout(resolve, 100));
		expect(session).not.toHaveBeenCalled();
		expect(owner.workers[0].terminated).toBe(false);
		owner.client.disconnect();
		stale.client.disconnect();
		session.mockRestore();
	});

	it('offline, an owner that does not answer in time is not taken to lack the library', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.setWorkingOffline(true);
		b.silence(true);
		// Unknown: the read goes on to ask the device, which waits or says it is unavailable.
		expect(await second.client.onDevice('1')).toBe(true);
		first.client.disconnect();
	});

	it('a caller that found the server unreachable waits for this tab’s own worker, online', async () => {
		const b = browser();
		b.flags.slow = true;
		const { client, workers } = b.tab('a');
		client.preconnect('7');
		await vi.waitFor(() => expect(types(workers[0] ?? ({ sent: [] } as never))).toContain('start'));
		expect(await client.onDevice('1')).toBe(false);
		expect(await client.onDevice('1', true)).toBe(true);
		client.disconnect();
	});
});
