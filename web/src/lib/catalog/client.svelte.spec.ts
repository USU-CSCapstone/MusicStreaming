import { beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import type { RealtimeServerMessage } from '$lib/api/types';
import { CatalogClient } from './client.svelte';
import { FakeClock } from './fake-clock';
import { PROTOCOL, type FromWorker, type LibraryState, type ToWorker } from './protocol';
import { CatalogRouter, Unanswered } from './router';

beforeEach(() => localStorage.clear());

/** What a library on a fake worker's device says of itself, unless a test changes it. */
const HERE: Record<string, LibraryState> = {
	'1': { onDevice: true, applying: false }
};

/** A worker the test plays the catalog for; it answers a query with the name of its tab. */
class FakeWorker {
	sent: ToWorker[] = [];
	terminated = false;
	onmessage: ((event: MessageEvent<FromWorker>) => void) | null = null;
	onerror: (() => void) | null = null;

	constructor(
		private tab: string,
		private silent: () => boolean,
		private clock: FakeClock,
		private flags: {
			busy: boolean;
			stalled: boolean;
			slow: boolean;
			mute: boolean;
			libraries: Record<string, LibraryState>;
		}
	) {}

	/** Records what the client sent, and answers as the real worker does. */
	postMessage(message: ToWorker) {
		this.sent.push(message);
		if (message.type === 'start') {
			this.say({ type: 'state', libraries: this.flags.libraries });
			if (this.flags.slow) this.clock.after(300, () => this.say({ type: 'started' }));
			else if (!this.flags.stalled) this.say({ type: 'started' });
		} else if (message.type === 'busy')
			this.say({ type: 'answer', id: message.id, result: this.flags.busy ? '1' : '' });
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

/**
 * Lets what is already on its way through the channel `name` and the locks arrive, so a test can
 * check that nothing more happened.
 */
async function flush(name: string) {
	for (let round = 0; round < 3; round++) {
		await navigator.locks.query();
		await new Promise<void>((resolve) => {
			const out = new BroadcastChannel(name);
			const back = new BroadcastChannel(name);
			back.onmessage = () => {
				out.close();
				back.close();
				resolve();
			};
			out.postMessage({ protocol: PROTOCOL, type: 'probe' });
		});
	}
}

/**
 * A set of tabs of one browser: they share a lock and a channel, and each has its own client and
 * workers.
 */
function browser() {
	const name = `catalog-test-${++tabs}-${Math.random()}`;
	let silent = false;
	const clock = new FakeClock();
	const all: FakeWorker[] = [];
	const flags = { busy: false, stalled: false, slow: false, mute: false, libraries: HERE };
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
			const client = new CatalogClient(
				() => {
					workers.push(new FakeWorker(tabName, () => silent, clock, flags));
					all.push(workers[workers.length - 1]);
					return workers[workers.length - 1] as unknown as Worker;
				},
				name,
				clock
			);
			return { client, workers };
		},
		/** The router of a worker of the queue or any other asker, on the browser's channel. */
		router() {
			return new CatalogRouter(name, 500, undefined, clock);
		},
		/** Makes every worker leave queries unanswered, as one that is closing does. */
		silence(on: boolean) {
			silent = on;
		},
		/** Has every worker say it is entering, or leaving, a call that holds it. */
		heavy(on: boolean) {
			for (const worker of all) worker.say({ type: 'heavy', on });
		},
		/** Has every worker say what its libraries stand at now. */
		libraries(libraries: Record<string, LibraryState>) {
			flags.libraries = libraries;
			for (const worker of all) worker.say({ type: 'state', libraries });
		},
		name,
		/** The time the tabs' timers run on, which passes only as a test advances it. */
		clock,
		/**
		 * Makes workers report a snapshot downloading, never say `started`, say it late, or never
		 * answer a sign-out.
		 */
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
		b.hear({ type: 'libraryChanged', libraryId: '1', position: 5 });
		b.hear({ type: 'accountChanged', position: 3 });

		expect(types(workers[0])).toEqual(['init', 'start', 'message', 'message', 'message']);
		expect(workers[0].sent[0]).toMatchObject({ type: 'init', user: '7' });
		client.disconnect();
	});

	it('a library is not on the device with no owner, and every tab learns the owner’s state, which changes as it does', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		expect(second.client.onDevice('1')).toBe(false);

		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));
		expect(first.client.onDevice('1')).toBe(true);
		expect(second.client.onDevice('2')).toBe(false);
		b.libraries({ '1': { onDevice: false, applying: false } });
		await vi.waitFor(() => expect(first.client.onDevice('1')).toBe(false));
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(false));
		// A library being updated is read from the server while online.
		b.libraries({ '1': { onDevice: true, applying: true } });
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(false));
		first.client.disconnect();
		second.client.disconnect();
	});

	it('onDevice asks no one: no request crosses the channel or reaches a worker', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		const requests: string[] = [];
		const probe = new BroadcastChannel(b.name);
		probe.onmessage = ({ data }) => data.type === 'request' && requests.push(data.request.type);
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));

		for (const { client } of [first, second]) client.onDevice('1');
		await flush(b.name);
		expect(requests).toEqual([]);
		expect(types(first.workers[0])).toEqual(['init', 'start']);
		probe.close();
		first.client.disconnect();
		second.client.disconnect();
	});

	it('an owner inside one long call, such as a snapshot install, is skipped at once by online reads, with no wait for the device deadline', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));

		b.heavy(true);
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(false));
		expect(first.client.onDevice('1')).toBe(false);
		// No time passed on the clock, and no request went to the worker.
		expect(types(first.workers[0])).toEqual(['init', 'start']);

		b.heavy(false);
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));
		first.client.disconnect();
		second.client.disconnect();
	});

	it('an owner that let go during a long call does not call itself busy when it owns the catalog again', async () => {
		const b = browser();
		const { client, workers } = b.tab('owner');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		b.heavy(true);
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(false));
		client.disconnect();
		await vi.waitFor(() => expect(workers[0].terminated).toBe(true));
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(workers).toHaveLength(2));
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(true));
		client.disconnect();
	});

	it('an owner never doubts itself: its own silent worker leaves its reads on the device', async () => {
		const b = browser();
		const { client, workers } = b.tab('owner');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(true));
		b.silence(true);
		const read = client.query('1', 'listTracks').catch(() => {});
		await b.clock.advance(1500);
		await read;
		expect(client.onDevice('1')).toBe(true);
		client.disconnect();
	});

	it('an owner of another account is no owner to this tab: its device is not read, though it serves', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(first.client.onDevice('1')).toBe(true));
		second.client.connect(b.realtime, '8');
		await flush(b.name);
		expect(second.client.onDevice('1')).toBe(false);
		first.client.disconnect();
		second.client.disconnect();
	});

	it('an owner closed without warning: an online read is answered by the server after the device deadline, and is not asked again until the owner says it is there', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));

		// The owner's page froze after its last word: it still says it serves, and answers nothing.
		b.silence(true);
		const outcome = vi.fn();
		const believed = vi.fn();
		const read = second.client.query('1', 'listTracks').then(outcome, (error) => {
			outcome(error);
			// The moment the read fails, before anything else is heard: the reads that follow go to the
			// server at once.
			believed(second.client.onDevice('1'));
		});
		await b.clock.advance(1499);
		expect(outcome).not.toHaveBeenCalled();
		await b.clock.advance(1);
		await read;
		expect(outcome.mock.calls[0][0]).toBeInstanceOf(Unanswered);
		expect(believed).toHaveBeenCalledWith(false);

		// An owner that is alive says so when asked, and is believed again.
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));
		first.client.disconnect();
		second.client.disconnect();
	});

	it('an owner of another protocol is no owner: reads go to the server, and the app says it needs updating', async () => {
		const b = browser();
		const { client } = b.tab('a');
		const old = new BroadcastChannel(b.name);
		expect(client.outdated).toBe(false);
		// What a version 1 owner says: no protocol, and its state would read as serving.
		old.postMessage({ type: 'owner' });
		old.postMessage({ type: 'state', state: { serving: true, libraries: HERE } });
		old.postMessage({ protocol: 1, type: 'state', state: { serving: true, libraries: HERE } });
		await vi.waitFor(() => expect(client.outdated).toBe(true));
		expect(client.onDevice('1')).toBe(false);
		old.close();
	});

	it('an owner ignores a tab of another protocol that asks for the catalog', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		const old = new BroadcastChannel(b.name);
		old.postMessage({
			type: 'request',
			id: 'old',
			request: { type: 'query', library: '1', name: 'x', params: {} }
		});
		// Heard in order: once this answer is back, the old request was heard and not answered.
		await b.router().ask({ type: 'query', library: '1', name: 'y', params: {} });
		expect(workers[0].sent.filter((m) => m.type === 'query')).toHaveLength(1);
		old.close();
		client.disconnect();
	});

	it('a request’s own timeout replaces the router’s', async () => {
		const b = browser();
		const router = new CatalogRouter(b.name, 5000, undefined, b.clock);
		const outcome = vi.fn();
		const asked = router
			.ask({ type: 'query', library: '1', name: 'x', params: {} }, 100)
			.catch(outcome);
		await b.clock.advance(100);
		await asked;
		expect(outcome.mock.calls[0][0]).toBeInstanceOf(Unanswered);
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

	it('a tab restored from the cache asks the owner for its state again, which it may have missed', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(async () => expect(await waiting(b.name)).toBe(1));
		await flush(b.name);
		const asked: string[] = [];
		const channel = new BroadcastChannel(b.name);
		onTestFinished(() => channel.close());
		channel.onmessage = ({ data }) => asked.push(data.type);

		dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
		await vi.waitFor(() => expect(asked).toContain('who'));
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
		onTestFinished(() => hidden.mockRestore());
		b.flags.busy = true;
		document.dispatchEvent(new Event('visibilitychange'));
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('busy'));
		await flush(b.name);
		expect(first.workers[0].terminated).toBe(false);

		b.flags.busy = false;
		document.dispatchEvent(new Event('visibilitychange'));
		await vi.waitFor(() => expect(first.workers[0].terminated).toBe(true));
		hidden.mockReturnValue(false);
		first.client.disconnect();
		second.client.disconnect();
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
		await b.clock.advance(19_000);
		expect(second.workers).toHaveLength(0);

		await b.clock.advance(2_000);
		await vi.waitFor(() => expect(second.workers).toHaveLength(1));
		expect(first.workers[0].terminated).toBe(true);
		first.client.disconnect();
		second.client.disconnect();
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
		await flush(b.name);
		// Each channel hears a message in its own time, so the waiting tab may take the lock before
		// the sign-out reaches it; it then answers the sign-out too, and leaves all the same.
		await vi.waitFor(() => expect(waiting.workers.every((w) => w.terminated)).toBe(true));
		const { held = [], pending = [] } = await navigator.locks.query();
		expect([...held, ...pending].filter((lock) => lock.name === b.name)).toEqual([]);
	});

	it('a request nobody answers fails after the timeout', async () => {
		const b = browser();
		const outcome = b
			.router()
			.ask({ type: 'query', library: '1', name: 'x', params: {} })
			.then(
				() => 'answered',
				(error) => error
			);
		await b.clock.advance(500);
		expect(await outcome).toBeInstanceOf(Unanswered);
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

	it('sign-out empties every account’s image cache and queue lists, and what the app starts from', async () => {
		for (const user of ['1', '2']) {
			const images = await caches.open(`images-${user}`);
			await images.put('/api/v1/libraries/1/images/9?size=96', new Response('jpeg'));
			const lists = await caches.open(`queue-lists-${user}`);
			await lists.put('/api/v1/me/queue/lists/abc', new Response('{}'));
		}
		localStorage.setItem('jewelcase-owner', '1');
		localStorage.setItem('jewelcase-user:1', '{"me":{"id":"1"}}');
		const b = browser();
		await b.tab('a').client.signOut();
		for (const user of ['1', '2']) {
			expect(await caches.has(`images-${user}`)).toBe(false);
			expect(await caches.has(`queue-lists-${user}`)).toBe(false);
		}
		expect(localStorage.getItem('jewelcase-owner')).toBeNull();
		expect(localStorage.getItem('jewelcase-user:1')).toBeNull();
	});

	it('sign-out tells the service worker that nobody is signed in', async () => {
		const postMessage = vi.fn();
		const spy = vi
			.spyOn(navigator.serviceWorker, 'getRegistration')
			.mockResolvedValue({ active: { postMessage } } as never);
		try {
			await browser().tab('a').client.signOut();
			expect(postMessage).toHaveBeenCalledWith({ type: 'user', user: null });
		} finally {
			spy.mockRestore();
		}
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
		await flush(b.name);
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
		await flush(b.name);
		expect(owner.workers[0].terminated).toBe(false);
		expect(JSON.parse(await owner.client.query('1', 'listTracks'))).toEqual({ tab: 'owner' });
		owner.client.disconnect();
		stale.client.disconnect();
		session.mockRestore();
	});

	it('a sign-out the local worker never answers is given up on after the router’s timeout', async () => {
		const b = browser();
		b.flags.mute = true;
		const { client, workers } = b.tab('a');
		let done = false;
		const out = client.signOut().then(() => (done = true));
		await vi.waitFor(() => expect(workers).toHaveLength(1));
		await vi.waitFor(() => expect(types(workers[0])).toContain('signOut'));
		await b.clock.advance(9_000);
		expect(done).toBe(false);
		await b.clock.advance(2_000);
		await out;
		expect(workers[0].terminated).toBe(true);
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
		let done = false;
		const out = first.client.signOut().then(() => (done = true));
		// Time passes on the tabs' clock until the sign-out is over.
		let elapsed = 0;
		for (; !done && elapsed < 10_000; elapsed += 50) await b.clock.advance(50);
		await out;
		expect(elapsed).toBeLessThan(5_000);
		second.client.disconnect();
	});

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

	it('once this tab serves the catalog itself, its reads go to the device again after an owner it doubted', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));
		b.silence(true);
		const believed = vi.fn();
		const read = second.client
			.query('1', 'listTracks')
			.catch(() => believed(second.client.onDevice('1')));
		await b.clock.advance(1500);
		await read;
		expect(believed).toHaveBeenCalledWith(false);

		b.silence(false);
		first.client.disconnect();
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));
		second.client.disconnect();
	});

	it('a tab that took the catalog over keeps its own state over the last owner’s, heard after it', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(first.client.onDevice('1')).toBe(true));
		second.client.connect(b.realtime, '7');
		first.client.disconnect();
		await vi.waitFor(() => expect(second.workers[0]?.terminated).toBe(false));
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));

		// The first owner's goodbye, which another channel may bring after this tab's own state.
		const late = new BroadcastChannel(b.name);
		late.postMessage({
			protocol: PROTOCOL,
			type: 'state',
			state: { protocol: PROTOCOL, user: '7', serving: false, heavy: false, libraries: {} }
		});
		late.close();
		await flush(b.name);
		expect(second.client.onDevice('1')).toBe(true);
		second.client.disconnect();
	});

	it('every tab counts a change the owner announces, for a page to ask what its read missed', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		const [one, two] = [first.client.changes, second.client.changes];
		first.workers[0].say({ type: 'changed', libraryId: '1', kinds: ['hidden'] });
		await vi.waitFor(() => expect(first.client.changedSince(one, '1', ['hidden'])).toBe(true));
		await vi.waitFor(() => expect(second.client.changedSince(two, '1', ['hidden'])).toBe(true));
		first.client.disconnect();
		second.client.disconnect();
	});
});
