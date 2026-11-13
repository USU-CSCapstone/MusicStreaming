import { beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import type { RealtimeServerMessage } from '$lib/api/types';
import { connectivity } from '$lib/connectivity.svelte';
import { deviceLibraries, rememberDevice } from '$lib/last-known';
import { toaster } from '$lib/toast.svelte';
import { IdbPort } from '$lib/sync/store';
import { CatalogClient } from './client.svelte';
import { FakeClock } from './fake-clock';
import { PROTOCOL, type FromWorker, type LibraryState, type ToWorker } from './protocol';
import { CatalogRouter, Unanswered } from './router';

beforeEach(() => localStorage.clear());

/** What a library on a fake worker's device says of itself, unless a test changes it. */
const HERE: Record<string, LibraryState> = {
	'1': { onDevice: true, applying: false, indexed: true }
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
			/**
			 * The libraries it says it found a file for; by default those it holds, and none said when
			 * null, as a failed start says.
			 */
			found?: string[] | null;
		}
	) {}

	/** Records what the client sent, and answers as the real worker does. */
	postMessage(message: ToWorker) {
		this.sent.push(message);
		if (message.type === 'start') {
			this.say({ type: 'state', libraries: this.flags.libraries });
			const found =
				this.flags.found === null
					? undefined
					: (this.flags.found ?? Object.keys(this.flags.libraries));
			if (this.flags.slow) this.clock.after(300, () => this.say({ type: 'started', found }));
			else if (!this.flags.stalled) this.say({ type: 'started', found });
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
	const flags = {
		busy: false,
		stalled: false,
		slow: false,
		mute: false,
		libraries: HERE,
		found: undefined as string[] | null | undefined
	};
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
					workers.push(new FakeWorker(tabName, () => silent, clock, flags));
					all.push(workers[workers.length - 1]);
					return workers[workers.length - 1] as unknown as Worker;
				},
				name,
				() => view.hidden,
				clock
			);
			/** Shows or hides the tab's page and fires the event the client listens to. */
			const change = (hidden: boolean) => {
				view.hidden = hidden;
				document.dispatchEvent(new Event('visibilitychange'));
			};
			return { client, workers, hide: () => change(true), show: () => change(false) };
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

	it('tells the owner’s worker whether the device is working offline, as it starts and when it changes', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		connectivity.setWorkOffline(true);
		onTestFinished(() => connectivity.setWorkOffline(false));
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
		connectivity.setWorkOffline(true);
		onTestFinished(() => connectivity.setWorkOffline(false));
		rememberDevice('7', ['1']);
		client.preconnect('7');
		// Offline and asked at once, before the election has the lock: it waits rather than saying no.
		expect(await client.waitForDevice('1')).toBe(true);
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

	it('a tab whose worker failed finds no library on the device, and an offline read gives up on it after the cold start wait', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		rememberDevice('7', ['1']);
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		workers[0].onerror?.();
		// The lock is given back once the failed worker is stopped.
		await vi.waitFor(async () =>
			expect((await navigator.locks.query()).held?.some((lock) => lock.name === b.name)).toBe(false)
		);
		expect(client.onDevice('1')).toBe(false);
		const settled = vi.fn();
		const wait = client.waitForDevice('1').then(settled);
		await b.clock.advance(4999);
		expect(settled).not.toHaveBeenCalled();
		await b.clock.advance(1);
		await wait;
		expect(settled).toHaveBeenCalledWith(false);
	});

	it('a tab that joined early can still sign out', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		client.preconnect('7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		await client.signOut();
		expect(types(workers.at(-1)!)).toContain('signOut');
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
		b.libraries({ '1': { onDevice: false, applying: false, indexed: false } });
		await vi.waitFor(() => expect(first.client.onDevice('1')).toBe(false));
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(false));
		// A library being updated is read from the server while online.
		b.libraries({ '1': { onDevice: true, applying: true, indexed: true } });
		await vi.waitFor(() => expect(second.client.indexed('1')).toBe(true));
		expect(second.client.onDevice('1')).toBe(false);
		first.client.disconnect();
		second.client.disconnect();
	});

	it('onDevice and indexed ask no one: no request crosses the channel or reaches a worker', async () => {
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

		for (const { client } of [first, second]) {
			client.onDevice('1');
			client.indexed('1');
			await client.waitForDevice('1');
		}
		await flush(b.name);
		expect(requests).toEqual([]);
		expect(types(first.workers[0])).toEqual(['init', 'start']);
		probe.close();
		first.client.disconnect();
		second.client.disconnect();
	});

	it('a library’s index is built only as the owner says, and with no owner it is not, never an error', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		expect(second.client.indexed('1')).toBe(false);

		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		// Out of sight, so it only listens and never takes the catalog when the first lets go.
		second.hide();
		second.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(second.client.indexed('1')).toBe(true));
		b.libraries({ '1': { onDevice: true, applying: false, indexed: false } });
		await vi.waitFor(() => expect(second.client.indexed('1')).toBe(false));
		// An owner that lets go says so, whatever it held.
		b.libraries(HERE);
		await vi.waitFor(() => expect(second.client.indexed('1')).toBe(true));
		first.client.disconnect();
		await vi.waitFor(() => expect(second.client.indexed('1')).toBe(false));
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
		expect(second.client.indexed('1')).toBe(false);
		expect(first.client.onDevice('1')).toBe(false);
		// Offline there is no server to skip to: the device is still waited for.
		expect(await second.client.waitForDevice('1')).toBe(true);
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

	it('the libraries kept for the next launch are the ones on the device, under the owner’s account', async () => {
		const b = browser();
		b.flags.libraries = {
			'1': { onDevice: true, applying: false, indexed: false },
			'2': { onDevice: false, applying: false, indexed: false }
		};
		const { client, workers } = b.tab('owner');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0])).toContain('start'));
		await vi.waitFor(() => expect(localStorage.getItem('jewelcase-device:7')).toBe('["1"]'));
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
		expect(second.client.indexed('1')).toBe(false);
		// Kept as held for its own account, it waits for an owner of its own; this one is not it.
		rememberDevice('8', ['1']);
		expect(await second.client.waitForDevice('1')).toBe(false);
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

	it('an owner that is silent while the device is offline is waited for, not given up on after the device deadline', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));
		second.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(second.client.onDevice('1')).toBe(true));
		connectivity.setWorkOffline(true);
		onTestFinished(() => connectivity.setWorkOffline(false));

		b.silence(true);
		expect(await second.client.waitForDevice('1')).toBe(true);
		const outcome = vi.fn();
		const read = second.client.query('1', 'listTracks', {}, true).then(outcome, outcome);
		await b.clock.advance(9999);
		expect(outcome).not.toHaveBeenCalled();
		await b.clock.advance(1);
		await read;
		expect(outcome.mock.calls[0][0]).toBeInstanceOf(Unanswered);
		first.client.disconnect();
		second.client.disconnect();
	});

	it('a cold start offline waits for its own owner only for a library the device is kept as holding, and only for a while', async () => {
		const b = browser();
		const { client } = b.tab('a');
		// The owner's worker never comes up.
		b.flags.stalled = true;
		rememberDevice('7', ['1']);
		client.preconnect('7');

		// Not kept as held: no wait.
		expect(await client.waitForDevice('2')).toBe(false);
		const settled = vi.fn();
		const held = client.waitForDevice('1').then(settled);
		await b.clock.advance(4999);
		expect(settled).not.toHaveBeenCalled();
		await b.clock.advance(1);
		await held;
		expect(settled).toHaveBeenCalledWith(false);
		client.disconnect();
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
		expect(client.indexed('1')).toBe(false);
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
		const asked = router.ask({ type: 'prepareIndex', library: '1' }, 100).catch(outcome);
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

	it('a tab restored from the cache or unfrozen asks the owner for its state again, which it may have missed', async () => {
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
		asked.length = 0;
		document.dispatchEvent(new Event('resume'));
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

	it('an owner hidden with a waiting tab in view hands over, and wants the files back when shown', async () => {
		const b = browser();
		const first = b.tab('first');
		const second = b.tab('second');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(first.workers[0])).toContain('start'));

		// No one waits: nothing changes.
		first.hide();
		await flush(b.name);
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
		await flush(b.name);
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
		await flush(b.name);
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
		probe.postMessage({ protocol: PROTOCOL, type: 'turn', kind: 'want' });
		await flush(b.name);
		expect(first.workers[0].terminated).toBe(false);

		first.hide();
		probe.postMessage({ protocol: PROTOCOL, type: 'turn', kind: 'want' });
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
		await flush(b.name);
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
		// A frozen page runs no tasks: nothing but microtasks may stand between this and the end of its
		// ownership.
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
		await flush(b.name);
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

	it('a tab alone whose worker never starts tries again after giving the lock up, so the catalog is still owned', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		b.flags.stalled = true;
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0] ?? ({ sent: [] } as never))).toContain('start'));
		b.flags.stalled = false;

		await b.clock.advance(21_000);
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(true));
		expect(workers).toHaveLength(2);
		expect(workers[0].terminated).toBe(true);
		client.disconnect();
	});

	it('a tab that leaves after its worker never started does not join again', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		b.flags.stalled = true;
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0] ?? ({ sent: [] } as never))).toContain('start'));
		b.flags.stalled = false;

		// Signed out, or its layout gone, as the wait for the worker ends and the hold is let go of.
		await b.clock.advance(19_999);
		const passing = b.clock.advance(1);
		client.disconnect();
		await passing;
		await b.clock.advance(21_000);
		await flush(b.name);
		expect(workers).toHaveLength(1);
	});

	it('a tab that leaves and joins again while its worker starts ends up owning the catalog', async () => {
		const b = browser();
		const { client, workers } = b.tab('a');
		b.flags.stalled = true;
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(types(workers[0] ?? ({ sent: [] } as never))).toContain('start'));
		b.flags.stalled = false;

		// As the layout did on each Work offline toggle: out of the election and straight back in.
		client.disconnect();
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(true));
		await flush(b.name);
		expect(client.onDevice('1')).toBe(true);
		expect(workers.at(-1)!.terminated).toBe(false);
		client.disconnect();
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

	it('sign-out deletes the owner’s log, unsent work and all', async () => {
		localStorage.setItem('jewelcase-owner', '1');
		const port = new IdbPort('1');
		await port.put('mutations', { key: 'p', rev: 'r', owner: 'tab', createdAt: 1, tries: 0 });
		port.close();
		await browser().tab('a').client.signOut();
		expect((await indexedDB.databases()).map((db) => db.name)).not.toContain('jewelcase-outbox-1');
	});

	it('a session the server ended notes how many unsent plays and changes it discarded, which a chosen logout does not', async () => {
		for (const chosen of [false, true]) {
			sessionStorage.clear();
			localStorage.setItem('jewelcase-owner', '1');
			const port = new IdbPort('1');
			for (const key of ['a', 'b'])
				await port.put('mutations', { key, rev: 'r', owner: 'tab', createdAt: 1, tries: 0 });
			// One set aside is as unsent.
			await port.put('dead', { key: 'c', rev: 'r', owner: 'tab', createdAt: 1, tries: 20 });
			port.close();
			await browser().tab('a').client.signOut(chosen);
			expect(sessionStorage.getItem('jewelcase-discarded'), String(chosen)).toBe(
				chosen ? null : '3'
			);
		}
	});

	it('a session that ended with nothing unsent notes nothing', async () => {
		sessionStorage.clear();
		localStorage.setItem('jewelcase-owner', '1');
		await browser().tab('a').client.signOut();
		expect(sessionStorage.getItem('jewelcase-discarded')).toBeNull();
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

	it('says once that a library the device kept has no file at start', async () => {
		toaster.dismiss();
		// The device last held libraries 1 and 2; only 1 has a file now.
		rememberDevice('7', ['1', '2']);
		const b = browser();
		const first = b.tab('a');
		first.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(toaster.current?.message).toContain('no longer on this device'));
		expect(toaster.current?.message).toBe(
			'A music library is no longer on this device and will be downloaded again.'
		);
		// What is kept is what is held now, so the next start has nothing to report.
		expect(deviceLibraries('7')).toEqual(new Set(['1']));
		toaster.dismiss();
		first.client.disconnect();

		const next = b.tab('b');
		next.client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(next.client.onDevice('1')).toBe(true));
		expect(toaster.current).toBeNull();
		next.client.disconnect();
	});

	it('says nothing of a kept library whose file the worker found and deleted itself, as after a schema upgrade', async () => {
		toaster.dismiss();
		rememberDevice('7', ['1', '2']);
		const b = browser();
		// Found a file for 2 and dropped it: it is no longer on the device, and nothing went missing.
		b.flags.found = ['1', '2'];
		const { client } = b.tab('a');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(true));
		expect(toaster.current).toBeNull();
		client.disconnect();
	});

	it('says nothing when the worker’s start failed, whatever it holds', async () => {
		toaster.dismiss();
		rememberDevice('7', ['1', '2']);
		const b = browser();
		b.flags.found = null;
		const { client } = b.tab('a');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(true));
		expect(toaster.current).toBeNull();
		client.disconnect();
	});

	it('gives an account’s page nothing of another account’s owner', async () => {
		const b = browser();
		const owner = b.tab('a');
		owner.client.connect(b.realtime, '8');
		await vi.waitFor(() => expect(owner.client.libraries).toEqual(HERE));
		// Another tab, signed in as 7, hears the owner of 8's state and takes none of it.
		const other = b.tab('b');
		other.client.preconnect('7');
		await flush(b.name);
		expect(other.client.libraries).toEqual({});
		expect(other.client.onDevice('1')).toBe(false);
		owner.client.disconnect();
		other.client.disconnect();
	});

	it('says nothing at start of a library that was never kept or is still there', async () => {
		toaster.dismiss();
		rememberDevice('7', ['1']);
		const b = browser();
		const { client } = b.tab('a');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(true));
		expect(toaster.current).toBeNull();
		client.disconnect();
	});

	it('shows the owner’s account page what the device holds of each library, and what it left for want of room', async () => {
		const b = browser();
		const { client } = b.tab('a');
		client.connect(b.realtime, '7');
		await vi.waitFor(() => expect(client.libraries).toEqual(HERE));
		const short = {
			onDevice: false,
			applying: false,
			indexed: false,
			noRoom: { need: 9, free: 3 }
		};
		b.libraries({ ...HERE, '2': short });
		await vi.waitFor(() => expect(client.libraries['2']).toEqual(short));
		client.disconnect();
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

	it('a stale tab’s index request builds nothing for the owner’s account', async () => {
		const session = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ id: '8' }));
		const b = browser();
		const owner = b.tab('owner');
		const stale = b.tab('stale');
		owner.client.connect(b.realtime, '8');
		await vi.waitFor(() => expect(types(owner.workers[0])).toContain('start'));
		stale.client.connect(b.realtime, '7');

		stale.client.prepareIndex('1');
		await vi.waitFor(() => expect(session).toHaveBeenCalled());
		await flush(b.name);
		owner.client.prepareIndex('1');
		await vi.waitFor(() => expect(types(owner.workers[0])).toContain('prepareIndex'));
		expect(owner.workers[0].sent.filter((m) => m.type === 'prepareIndex')).toHaveLength(1);
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

	it('online, a read does not wait for this tab’s own worker to start', async () => {
		const b = browser();
		b.flags.slow = true;
		const { client, workers } = b.tab('a');
		client.preconnect('7');
		await vi.waitFor(() => expect(types(workers[0] ?? ({ sent: [] } as never))).toContain('start'));
		// Not serving yet: the answer is no, at once, so the server answers the read.
		expect(client.onDevice('1')).toBe(false);
		await b.clock.advance(300);
		await vi.waitFor(() => expect(client.onDevice('1')).toBe(true));
		client.disconnect();
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

	it('an owner that has to check who the session is makes no request while working offline', async () => {
		const session = vi.spyOn(globalThis, 'fetch').mockResolvedValue(Response.json({ id: '8' }));
		const b = browser();
		const owner = b.tab('owner');
		const stale = b.tab('stale');
		connectivity.setWorkOffline(true);
		onTestFinished(() => connectivity.setWorkOffline(false));
		owner.client.connect(b.realtime, '8');
		await vi.waitFor(() => expect(types(owner.workers[0])).toContain('start'));
		stale.client.connect(b.realtime, '7');

		await expect(stale.client.query('1', 'listTracks')).rejects.toThrow(/another account/);
		await flush(b.name);
		expect(session).not.toHaveBeenCalled();
		expect(owner.workers[0].terminated).toBe(false);
		owner.client.disconnect();
		stale.client.disconnect();
		session.mockRestore();
	});

	it('a caller that found the server unreachable waits for this tab’s own worker, online', async () => {
		const b = browser();
		b.flags.slow = true;
		const { client, workers } = b.tab('a');
		rememberDevice('7', ['1']);
		client.preconnect('7');
		await vi.waitFor(() => expect(types(workers[0] ?? ({ sent: [] } as never))).toContain('start'));
		expect(client.onDevice('1')).toBe(false);
		const waited = client.waitForDevice('1');
		await b.clock.advance(300);
		expect(await waited).toBe(true);
		client.disconnect();
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
