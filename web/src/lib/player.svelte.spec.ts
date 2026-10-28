import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	PlayReport,
	PlayerState,
	QueueEdit,
	RealtimeServerMessage,
	StartQueue,
	Track
} from '$lib/api/types';
import type { ToWorker } from '$lib/queue/protocol';
import { remember } from '$lib/queue/summaries';

const fake = vi.hoisted(() => ({
	/** Tracks whose fetch is held back, by ID; any other arrives at once, a minute long. */
	tracks: new Map<string, Promise<unknown>>(),
	/** Tracks as the catalog has them, by ID, in place of that default. */
	catalog: new Map<string, unknown>(),
	/** The edits and starts the queue sent to the server. */
	edits: [] as QueueEdit[],
	starts: [] as StartQueue[],
	/** Whether the server has no queue to start, as the dev mock has none. */
	noQueue: false,
	/** Holds the server's answer to a start until it settles. */
	holdStart: null as Promise<unknown> | null,
	/** The tracks a start's list query reads to its end. */
	list: [] as string[],
	/** The audio's playback info; by default it never arrives, so nothing plays. */
	info: null as (() => Promise<unknown>) | null,
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	/** Libraries the account no longer reaches, whose lookups the server answers with 404. */
	unreached: new Set<string>(),
	/** Whether messages to the server fail to go. */
	offline: false,
	sent: [] as {
		type: string;
		report: { status: string; positionMs: number; start: boolean; itemId: string };
		result: unknown;
	}[],
	plays: [] as PlayReport[],
	realtime: {
		deviceId: '1',
		connected: true,
		clockOffset: 0,
		on: (listener: (message: RealtimeServerMessage) => void) => fake.listeners.push(listener),
		send(message: never) {
			if (fake.offline) return false;
			fake.sent.push(message);
			return true;
		}
	}
}));

vi.mock('$lib/realtime.svelte', () => ({ realtime: fake.realtime }));
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	getTracks: async (f: never, library: string, ids: string[]) => {
		if (fake.unreached.has(library)) {
			const refused = async () => new Response('{"status":404}', { status: 404 });
			return (await actual<typeof import('$lib/api/client')>()).getTracks(refused, library, ids);
		}
		const items = await Promise.all(
			ids.map((id) => fake.tracks.get(id) ?? fake.catalog.get(id) ?? track(id, 60))
		);
		return { items, total: items.length, nextCursor: null };
	},
	getWaveform: async () => null,
	recordPlays: async (_: unknown, plays: PlayReport[]) => void fake.plays.push(...plays),
	getPlaybackInfo: () => fake.info?.() ?? new Promise(() => {})
}));

const track = (id: string, seconds: number, availability = 'available') =>
	({
		id,
		durationUs: seconds * 1e6,
		availability,
		album: {},
		artists: []
	}) as unknown as Track;

// The queue as the app has it, in a worker, here with the worker's store in the page and a
// server that takes every start and edit.
vi.mock('$lib/queue/client.svelte', async (actual) => {
	const real = await actual<typeof import('$lib/queue/client.svelte')>();
	const { QueueStore } = await import('$lib/queue/store');
	const wasm = await import('$lib/core-wasm/jewelcase_core_wasm.js');
	await wasm.default();
	let version = 0;
	let serverId = '';
	let serverContext: object = {};
	let server: import('$lib/core-wasm/jewelcase_core_wasm.js').Queue | null = null;
	/** A response with this status and, if given, this JSON body. */
	const answer = (status: number, body?: object) =>
		new Response(body && JSON.stringify(body), { status });
	const store = new QueueStore({
		api: '/api/v1',
		fetch: async (url, init) => {
			const body = JSON.parse(String(init?.body ?? 'null'));
			if (String(url).endsWith('/me/queue/edits')) {
				fake.edits.push(body.edit);
				server?.apply(JSON.stringify(body.edit));
				return answer(200, { version: ++version });
			}
			if (init?.method === 'POST') {
				fake.starts.push(body);
				await fake.holdStart;
				if (fake.noQueue) {
					server = null;
					return answer(404);
				}
				// As the server starts it: a list's query reads as `fake.list`.
				const { query, tracks, ...source } = body.source;
				const list = query ? fake.list : tracks;
				const first = query ? list.indexOf(body.startTrackId) : body.start;
				server = wasm.Queue.start(JSON.stringify({ ...source, tracks: list }), first);
				serverId = body.queueId;
				serverContext = body.context;
				version = 0;
				return answer(204);
			}
			if (!server) return answer(404);
			const state = JSON.parse(server.snapshot());
			return answer(200, { queueId: serverId, version, context: serverContext, state });
		},
		changed: (summary) => worker.onmessage?.({ data: { type: 'state', summary } } as MessageEvent),
		refused: () => {}
	});
	const worker = {
		onmessage: null as ((event: MessageEvent) => void) | null,
		postMessage(message: ToWorker) {
			if (message.type === 'start') store.start(message.args);
			else if (message.type === 'edit') store.edit(message.edit);
			else if (message.type === 'ask') {
				const value = store.ask(message.question);
				worker.onmessage?.({ data: { type: 'answer', id: message.id, value } } as MessageEvent);
			}
		},
		terminate() {}
	};
	const queue = new real.QueueClient(() => worker as unknown as Worker);
	queue.connect({ on: () => () => {} });
	return { ...real, queue };
});

// Before the player sets its handlers, so a test can act as the lock screen does.
const setAction = vi.spyOn(navigator.mediaSession, 'setActionHandler');
const action = (name: MediaSessionAction) =>
	setAction.mock.calls.findLast(([a]) => a === name)![1]!({ action: name });

const { player } = await import('./player.svelte');
const { controls } = await import('./controls.svelte');
const { queue } = await import('$lib/queue/client.svelte');
const { playback } = await import('$lib/playback.svelte');

const send = (message: RealtimeServerMessage) =>
	fake.listeners.forEach((listener) => listener(message));

/** The queue's items in play order: the current, the Manual Queue, then Up Next. */
let items: string[] = [];
const itemsNow = async () => [
	queue.current!,
	...(await queue.window('manual', 0, 100)),
	...(await queue.window('upcoming', 0, 100))
];

/** Starts a queue of these tracks without the player's help, as another view of it would. */
async function hold(ids: string[], start = 0) {
	queue.start({ context: { type: 'library' }, libraryId: '11', tracks: ids, start });
	await vi.waitFor(() => expect(queue.current).toMatch(/\.\d+$/));
	items = await itemsNow();
}

/** Sends the account's state, playing the first queue item on device 2, with `changes`. */
function state(changes: Partial<PlayerState> = {}) {
	send({
		type: 'player',
		state: {
			status: 'playing',
			activeDeviceId: '2',
			itemId: items[0] ?? '0',
			positionMs: 10_000,
			positionAt: new Date().toISOString(),
			volume: 0.2,
			...changes
		}
	});
}

const transfer = (play: boolean) =>
	send({
		type: 'execute',
		command: { type: 'transfer', commandId: crypto.randomUUID(), deviceId: '1', play }
	});

const result = () => vi.waitFor(() => fake.sent.find((m) => m.type === 'commandResult')!.result);

/** Another tab of this browser, whose playing makes this player let go of any it held. */
const otherTab = new BroadcastChannel('jewelcase.player');

beforeEach(async () => {
	otherTab.postMessage('started');
	// The last test's edits, still on their way, would be superseded by this one's start.
	await new Promise((resolve) => setTimeout(resolve, 30));
	fake.sent.length = 0;
	fake.plays.length = 0;
	fake.edits.length = 0;
	fake.starts.length = 0;
	fake.noQueue = false;
	fake.holdStart = null;
	fake.list = [];
	player.finished = false;
	playback.state = null;
	fake.offline = false;
	fake.unreached.clear();
	fake.tracks.clear();
	fake.catalog.clear();
	fake.info = null;
	items = [];
	player.libraryId = '11';
	player.volume = 0.8;
});

afterEach(() => vi.restoreAllMocks());

/** Lets the player's lookups and the queue's messages arrive. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 30));

describe('player', () => {
	it('takes playback over when another device moves it here', async () => {
		await hold(['41', '42']);
		state();
		transfer(true);
		expect(await result()).toMatchObject({ status: 'applied' });
		const [report] = fake.sent;
		expect(report.type, 'taken over before answering').toBe('progress');
		expect(report.report).toMatchObject({
			start: true,
			itemId: items[0],
			status: 'buffering',
			// Its own volume, not the other device's.
			volume: 0.8
		});
		expect(report.report.positionMs).toBeGreaterThanOrEqual(10_000);
		expect(player.current?.id).toBe('41');
		expect(fake.edits, 'the queue was already there').toEqual([]);
	});

	it('plays the item of the queue the state names, moving the queue there if it is elsewhere', async () => {
		await hold(['41', '42', '43']);
		state({ itemId: items[2] });
		transfer(true);
		expect(await result()).toMatchObject({ status: 'applied' });
		expect(player.current?.id).toBe('43');
		await vi.waitFor(() => expect(fake.edits).toEqual([{ type: 'jump', item: items[2] }]));
		expect(queue.current).toBe(items[2]);
	});

	it('picks up an item of another library than the one viewed, from the item’s own', async () => {
		await hold(['41']);
		player.libraryId = '12';
		state();
		transfer(true);
		expect(await result()).toMatchObject({ status: 'applied' });
		expect(player.current?.id).toBe('41');
	});

	it('says when the item is not in the queue this device holds', async () => {
		await hold(['41']);
		state({ itemId: 'someone-elses.3' });
		transfer(true);
		expect(await result()).toMatchObject({ status: 'failed' });
	});

	it('does not take back what another device took over meanwhile', async () => {
		let arrive: (track: Track) => void = () => {};
		fake.tracks.set('43', new Promise((resolve) => (arrive = resolve)));
		await hold(['43']);
		state();
		transfer(true);
		state({ activeDeviceId: '3' });
		arrive(track('43', 60));
		expect(await result()).toMatchObject({ status: 'failed' });
		expect(fake.sent.map((m) => m.type)).toEqual(['commandResult']);
	});

	it('starts no later than the end of the track', async () => {
		fake.catalog.set('45', track('45', 5));
		await hold(['44', '45'], 1);
		state({ positionMs: 30_000 });
		transfer(true);
		await result();
		expect(fake.sent[0].report.positionMs).toBe(5000);
	});

	it('waits paused on a track that cannot play, rather than playing the next', async () => {
		fake.info = () => Promise.reject(new Error('gone'));
		await hold(['47', '48']);
		state();
		transfer(false);
		await result();
		// For the failure to be handled.
		await settle();
		expect(player.current?.id).toBe('47');
		expect(fake.edits).toEqual([]);
	});

	it('stays the player shown once it runs out of tracks, until playback starts again', async () => {
		fake.info = () => Promise.reject(new Error('gone'));
		player.play([track('46', 60)], 0);
		// The server confirms the takeover before the track turns out to be the last.
		state({ status: 'paused', activeDeviceId: '1', itemId: queue.current! });
		await vi.waitFor(() => expect(player.finished).toBe(true));
		expect(fake.sent.at(-1)?.report, 'at the start of the last track').toMatchObject({
			status: 'stopped',
			positionMs: 0
		});
		state({ status: 'stopped', activeDeviceId: null, itemId: queue.current! });
		expect(controls.remote).toBe(false);

		void player.resume();
		expect(fake.sent.at(-1)?.report, 'loaded again, taking over').toMatchObject({
			status: 'buffering',
			positionMs: 0,
			start: true
		});
		// Its audio fails again.
		await vi.waitFor(() => expect(player.finished).toBe(true));

		state({ activeDeviceId: '2', itemId: queue.current! });
		expect(player.finished).toBe(false);
		state({ status: 'stopped', activeDeviceId: null, itemId: queue.current! });
		expect(controls.remote, 'stopped elsewhere').toBe(true);
	});

	it('shows a stop that is not its own, as one heard on reconnecting', async () => {
		fake.info = () => Promise.reject(new Error('gone'));
		player.play([track('49', 60)], 0);
		state({ status: 'paused', activeDeviceId: '1', itemId: queue.current! });
		await vi.waitFor(() => expect(player.finished).toBe(true));
		state({ status: 'stopped', activeDeviceId: null, itemId: 'elsewhere.0' });
		expect(player.finished).toBe(false);
	});

	it('stays paused when paused before the track has its audio', async () => {
		let arrive: (info: unknown) => void = () => {};
		fake.info = () => new Promise((resolve) => (arrive = resolve));
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		player.play([track('51', 60)], 0);
		await vi.waitFor(() => expect(player.current?.id).toBe('51'));
		expect(player.playing, 'shown playing at once').toBe(true);
		send({ type: 'execute', command: { type: 'pause', commandId: crypto.randomUUID() } });
		expect(await result()).toMatchObject({ status: 'applied' });
		expect(player.playing).toBe(false);
		arrive({ variant: 'original' });
		await new Promise((resolve) => setTimeout(resolve));
		expect(play).not.toHaveBeenCalled();
		play.mockRestore();
	});

	it('stays on a track paused while it starts, rather than playing the next', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		let abort = () => {};
		const play = vi
			.spyOn(HTMLMediaElement.prototype, 'play')
			.mockImplementation(
				() => new Promise((_, reject) => (abort = () => reject(new DOMException('', 'AbortError'))))
			);
		player.play([track('52', 60), track('53', 60)], 0);
		await vi.waitFor(() => expect(play).toHaveBeenCalled());
		player.toggle();
		// As the browser does to a play cut short by a pause.
		abort();
		await new Promise((resolve) => setTimeout(resolve));
		expect(player.current?.id).toBe('52');
		expect(play).toHaveBeenCalledOnce();
		expect(fake.edits).toEqual([]);
		play.mockRestore();
	});

	it('stays on a track the browser pauses while it starts, rather than playing the next', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		let abort = () => {};
		const play = vi
			.spyOn(HTMLMediaElement.prototype, 'play')
			.mockImplementation(
				() => new Promise((_, reject) => (abort = () => reject(new DOMException('', 'AbortError'))))
			);
		player.play([track('54', 60), track('55', 60)], 0);
		await vi.waitFor(() => expect(play).toHaveBeenCalled());
		// As the browser does by itself, as when headphones are unplugged.
		(play.mock.contexts[0] as HTMLAudioElement).dispatchEvent(new Event('pause'));
		abort();
		await new Promise((resolve) => setTimeout(resolve));
		expect(player.current?.id).toBe('54');
		expect(player.playing).toBe(false);
		play.mockRestore();
	});

	it('starts where it was moved to before its audio arrived', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		await hold(['56']);
		state({ positionMs: 30_000 });
		transfer(true);
		await result();
		await vi.waitFor(() => expect(play).toHaveBeenCalled());
		const audio = play.mock.contexts[0] as HTMLAudioElement;
		player.seek(5);
		audio.dispatchEvent(new Event('loadedmetadata'));
		expect(audio.currentTime).toBe(5);
		play.mockRestore();
	});

	it('takes nothing over once another tab plays, though it could not say it played here', async () => {
		fake.offline = true;
		player.play([track('57', 60)], 0);
		const tab = new BroadcastChannel('jewelcase.player');
		await vi.waitFor(() => expect(player.current?.id).toBe('57'));
		tab.postMessage('started');
		await vi.waitFor(() => expect(player.playing).toBe(false));
		tab.close();
		fake.offline = false;
		send({ type: 'hello', deviceId: '1', serverTime: new Date().toISOString() });
		state();
		player.setVolume(0.5);
		await new Promise((resolve) => setTimeout(resolve, 300));
		expect(fake.sent).toEqual([]);
	});

	it('starts a new play when it plays again after the page was hidden', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		player.play([track('58', 60)], 0);
		await vi.waitFor(() => expect(play).toHaveBeenCalled());
		const audio = play.mock.contexts[0] as HTMLAudioElement;
		audio.dispatchEvent(new Event('playing'));
		dispatchEvent(new PageTransitionEvent('pagehide', { persisted: true }));
		// Back from the back/forward cache.
		audio.dispatchEvent(new Event('playing'));
		expect(fake.plays.map((p) => [p.trackId, p.end])).toEqual([
			['58', null],
			['58', 'stopped'],
			['58', null]
		]);
		expect(fake.plays[2].playId).not.toBe(fake.plays[0].playId);
		play.mockRestore();
	});

	it('leaves the lock screen to the device that took over', async () => {
		player.play([track('59', 60), track('60', 60)], 0);
		await vi.waitFor(() => expect(player.current?.id).toBe('59'));
		state({ activeDeviceId: '1', itemId: queue.current! });
		state();
		expect(player.playing).toBe(false);
		expect(navigator.mediaSession.metadata).toBeNull();

		// As a headset does as it connects, which must not take playback back here.
		fake.sent.length = 0;
		fake.edits.length = 0;
		action('play');
		action('nexttrack');
		await settle();
		expect(fake.sent).toEqual([]);
		expect(fake.edits).toEqual([]);
		expect(player.current?.id).toBe('59');
	});

	describe('from the queue', () => {
		/** The audio the player plays, once a track is playing. */
		async function playing() {
			fake.info = () => Promise.resolve({ variant: 'original' });
			return vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		}

		it('starts a queue of the tracks shown, or of the list a view pages through', async () => {
			const start = vi.spyOn(queue, 'start').mockImplementation(() => {});
			const shown = [track('61', 60), track('62', 60), track('63', 60, 'missing')];
			const context = { type: 'album', id: '9' } as const;
			player.play(shown, 1, context);
			expect(start).toHaveBeenLastCalledWith({
				context,
				libraryId: '11',
				tracks: ['61', '62', '63'],
				start: 1
			});

			const query = { type: 'tracks', sort: 'name', order: 'asc' } as const;
			// The first that can play, as the tracks shown are, with the list for the rest.
			player.play(shown, 2, undefined, query);
			expect(start).toHaveBeenCalledTimes(1);
			player.play(shown, 1, undefined, query);
			expect(start).toHaveBeenLastCalledWith({
				context: { type: 'library' },
				libraryId: '11',
				query,
				startTrackId: '62'
			});
			start.mockRestore();
		});

		it('plays the clicked track at once, and the next from its lookup', async () => {
			const play = await playing();
			fake.catalog.set('72', track('72', 30));
			player.play([track('71', 60), track('72', 30)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			expect(player.current?.id).toBe('71');

			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('72'));
			expect(fake.edits).toEqual([{ type: 'next' }]);
			expect(player.duration).toBe(30);
			play.mockRestore();
		});

		it('goes back through the queue, restarting a track that is under way', async () => {
			const play = await playing();
			player.play([track('73', 60), track('74', 60)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('74'));
			fake.edits.length = 0;

			player.position = 10;
			player.previous();
			expect(player.position, 'restarted').toBe(0);
			await settle();
			expect(fake.edits).toEqual([]);

			player.position = 1;
			player.previous();
			await vi.waitFor(() => expect(player.current?.id).toBe('73'));
			expect(fake.edits).toEqual([{ type: 'previous' }]);

			// With nothing before it, it only restarts.
			fake.edits.length = 0;
			player.position = 1;
			player.previous();
			await settle();
			expect(fake.edits).toEqual([]);
			expect(player.current?.id).toBe('73');
			play.mockRestore();
		});

		it('moves on when a track ends, and stops at the end of the queue', async () => {
			const play = await playing();
			player.play([track('75', 60), track('76', 60)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const audio = play.mock.contexts[0] as HTMLAudioElement;
			audio.dispatchEvent(new Event('ended'));
			await vi.waitFor(() => expect(player.current?.id).toBe('76'));
			audio.dispatchEvent(new Event('ended'));
			await vi.waitFor(() => expect(player.finished).toBe(true));
			expect(fake.edits, 'nothing to move to').toEqual([{ type: 'next' }]);
			expect(player.current?.id).toBe('76');
			play.mockRestore();
		});

		it('replays the track on Repeat One, and starts over with Repeat All', async () => {
			const play = await playing();
			player.play([track('77', 60)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const audio = play.mock.contexts[0] as HTMLAudioElement;
			audio.dispatchEvent(new Event('playing'));
			queue.edit({ type: 'repeat', mode: 'one' });
			await vi.waitFor(() => expect(queue.repeat).toBe('one'));
			fake.edits.length = 0;

			audio.dispatchEvent(new Event('ended'));
			await vi.waitFor(() => expect(play).toHaveBeenCalledTimes(2));
			expect(fake.edits, 'the queue stays where it is').toEqual([]);
			audio.dispatchEvent(new Event('playing'));
			expect(fake.plays.filter((p) => p.end === null).map((p) => p.trackId)).toEqual(['77', '77']);

			queue.edit({ type: 'repeat', mode: 'all' });
			await vi.waitFor(() => expect(queue.repeat).toBe('all'));
			fake.edits.length = 0;
			audio.dispatchEvent(new Event('ended'));
			await vi.waitFor(() => expect(fake.edits).toEqual([{ type: 'next' }]));
			await vi.waitFor(() => expect(play).toHaveBeenCalledTimes(3));
			// A new queue keeps the repeat mode, which the next test must not get.
			queue.edit({ type: 'repeat', mode: 'off' });
			await vi.waitFor(() => expect(queue.repeat).toBe('off'));
			play.mockRestore();
		});

		it('passes over tracks that cannot play with one jump, changing nothing else', async () => {
			const play = await playing();
			fake.catalog.set('82', track('82', 60, 'missing'));
			fake.catalog.set('83', track('83', 60, 'missing'));
			player.play(
				['81', '82', '83', '84', '85'].map((id) => track(id, 60)),
				0
			);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const all = await itemsNow();

			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('84'));
			const [, , , fourth] = all;
			expect(fake.edits).toEqual([{ type: 'next' }, { type: 'jump', item: fourth }]);
			// Passed in place: History has them in order, and Up Next is what followed.
			expect(await queue.window('history', 0, 10)).toEqual(all.slice(0, 3));
			expect(await queue.window('upcoming', 0, 10)).toEqual(all.slice(4));
			expect(await queue.window('manual', 0, 10)).toEqual([]);
			play.mockRestore();
		});

		it('goes back over missing tracks with previous, to the nearest that can play', async () => {
			const play = await playing();
			fake.catalog.set('92', track('92', 60, 'missing'));
			fake.catalog.set('93', track('93', 60, 'missing'));
			player.play(
				['90', '91', '92', '93', '94'].map((id) => track(id, 60)),
				0
			);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const all = await itemsNow();
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('91'));
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('94'));

			// Back from 94 passes 93 and 92, and stops at 91, not 90.
			fake.edits.length = 0;
			player.position = 0;
			player.previous();
			await vi.waitFor(() => expect(player.current?.id).toBe('91'));
			expect(fake.edits).toEqual([
				{ type: 'previous' },
				{ type: 'previous' },
				{ type: 'previous' }
			]);
			// What was passed is ahead again, in order.
			expect(await queue.window('upcoming', 0, 10)).toEqual(all.slice(2));
			expect(await queue.window('history', 0, 10)).toEqual(all.slice(0, 1));
		});

		it('restarts the track, changing nothing, when nothing behind it can play', async () => {
			const play = await playing();
			fake.catalog.set('112', track('112', 60, 'missing'));
			fake.catalog.set('113', track('113', 60, 'missing'));
			player.play(
				['111', '112', '113', '114'].map((id) => track(id, 60)),
				0
			);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('114'));
			queue.edit({
				type: 'add',
				source: { key: 'm', libraryId: '11', tracks: ['119'] },
				before: { type: 'manualEnd' }
			});
			await vi.waitFor(() => expect(queue.counts.manual).toBe(1));
			// The track went missing since it was looked up, which the cache does not know until it is asked again.
			remember('11', track('111', 60, 'missing'));
			const before = await itemsNow();
			const history = await queue.window('history', 0, 10);

			fake.edits.length = 0;
			player.position = 1;
			player.previous();
			await vi.waitFor(() => expect(player.position).toBe(0));
			await settle();
			expect(fake.edits).toEqual([]);
			expect(await itemsNow()).toEqual(before);
			expect(await queue.window('history', 0, 10)).toEqual(history);
			expect(await queue.window('manual', 0, 10)).toEqual(['m.0']);
			expect(player.current?.id).toBe('114');
			// A new queue carries the Manual Queue over, which the next test must not get.
			queue.edit({ type: 'remove', items: ['m.0'] });
			await vi.waitFor(() => expect(queue.counts.manual).toBe(0));
		});

		it('plays the item another device or client jumps to, but only where it holds playback', async () => {
			const play = await playing();
			player.play(
				['201', '202', '203', '204'].map((id) => track(id, 60)),
				0
			);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const all = await itemsNow();

			queue.edit({ type: 'jump', item: all[2] });
			await vi.waitFor(() => expect(player.current?.id).toBe('203'));
			await vi.waitFor(() => expect(play).toHaveBeenCalledTimes(2));

			// Paused here, it stays paused on the item it follows to.
			player.pause();
			queue.edit({ type: 'jump', item: all[3] });
			await vi.waitFor(() => expect(player.current?.id).toBe('204'));
			expect(player.playing).toBe(false);
			expect(play).toHaveBeenCalledTimes(2);

			// Another device took over: this one only shows the queue.
			player.toggle();
			state({ activeDeviceId: '2' });
			queue.edit({ type: 'jump', item: all[1] });
			await settle();
			expect(player.current?.id).toBe('204');
		});

		it('moves on from the first track of a list start once the server has the queue', async () => {
			const play = await playing();
			let release: (value?: unknown) => void = () => {};
			fake.holdStart = new Promise((resolve) => (release = resolve));
			fake.list = ['301', '302', '303'];
			const shown = fake.list.map((id) => track(id, 60));
			player.play(shown, 0, undefined, { type: 'tracks' });
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			expect(queue.settling).toBe(true);

			player.next();
			await settle();
			expect(player.finished, 'not the end of the queue').toBe(false);
			expect(player.current?.id).toBe('301');
			expect(fake.edits).toEqual([]);

			release();
			await vi.waitFor(() => expect(player.current?.id).toBe('302'));
			expect(fake.edits).toEqual([{ type: 'next' }]);
		});

		it('plays on when a track ends before the server has the queue of a list start', async () => {
			const play = await playing();
			let release: (value?: unknown) => void = () => {};
			fake.holdStart = new Promise((resolve) => (release = resolve));
			fake.list = ['321', '322', '323'];
			player.play(
				fake.list.map((id) => track(id, 60)),
				1,
				undefined,
				{ type: 'tracks' }
			);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const audio = play.mock.contexts[0] as HTMLAudioElement;
			audio.dispatchEvent(new Event('playing'));
			// As a browser ends a track: paused, then ended.
			audio.dispatchEvent(new Event('pause'));
			audio.dispatchEvent(new Event('ended'));

			release();
			await vi.waitFor(() => expect(player.current?.id).toBe('323'));
			await vi.waitFor(() => expect(play).toHaveBeenCalledTimes(2));
			expect(player.playing).toBe(true);
			audio.dispatchEvent(new Event('playing'));
			// A play is reported more than once, under its ID.
			const starts = new Map(
				fake.plays.filter((p) => p.end === null).map((p) => [p.playId, [p.trackId, p.origin]])
			);
			expect([...starts.values()]).toEqual([
				['322', 'context'],
				['323', 'context']
			]);
		});

		it('follows a change made elsewhere in the middle of stepping back', async () => {
			const play = await playing();
			fake.catalog.set('92', track('92', 60, 'missing'));
			fake.catalog.set('93', track('93', 60, 'missing'));
			player.play(
				['90', '91', '92', '93', '94'].map((id) => track(id, 60)),
				0
			);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const all = await itemsNow();
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('91'));
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('94'));

			// Only the first step back reaches the queue before another device jumps to 90.
			const edit = queue.edit.bind(queue);
			let steps = 0;
			vi.spyOn(queue, 'edit').mockImplementation((e) => {
				if (e.type !== 'previous' || ++steps === 1) edit(e);
			});
			player.position = 0;
			player.previous();
			await vi.waitFor(() => expect(steps).toBe(3));
			await settle();
			expect(player.current?.id, 'waiting for the run to land').toBe('94');

			edit({ type: 'jump', item: all[0] });
			// Not waiting for a landing that never came: what the queue says is what plays.
			await vi.waitFor(() => expect(player.current?.id).toBe('90'));
			expect(player.playing).toBe(true);
			vi.restoreAllMocks();
		});

		it("reports the server's item, not the one the list start began with", async () => {
			const play = await playing();
			fake.list = ['311', '312', '313'];
			player.play(
				fake.list.map((id) => track(id, 60)),
				1,
				undefined,
				{ type: 'tracks' }
			);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			await vi.waitFor(() => expect(queue.settling).toBe(false));
			// With no other report to carry it: the state must move off the placeholder itself.
			await vi.waitFor(() => {
				expect(fake.sent.findLast((m) => m.type === 'progress')?.report.itemId).toBe(queue.current);
			});
			expect(queue.current).toMatch(/\.1$/);
		});

		it('stops at the end when nothing after a missing track can play', async () => {
			const play = await playing();
			fake.catalog.set('96', track('96', 60, 'missing'));
			player.play([track('95', 60), track('96', 60, 'missing')], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			player.next();
			await vi.waitFor(() => expect(player.finished).toBe(true));
			expect(fake.edits.filter((e) => e.type === 'jump')).toEqual([]);
			play.mockRestore();
		});

		it('ignores what the audio of the track before reports once the next is chosen', async () => {
			fake.info = () => Promise.resolve({ variant: 'original' });
			const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
			player.play([track('141', 60), track('142', 60)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const audio = play.mock.contexts[0] as HTMLAudioElement;
			// Next is chosen before the first track's audio says it plays; the next track's audio is
			// still on its way.
			fake.info = () => new Promise(() => {});
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('142'));
			// What the first track's audio had queued: that it plays, and that it was paused.
			audio.dispatchEvent(new Event('playing'));
			audio.dispatchEvent(new Event('pause'));
			await settle();
			expect(fake.plays).toEqual([]);
			expect(player.playing, 'still on its way, not paused').toBe(true);
			play.mockRestore();
		});

		it('records plays with the context of the queue, also after taking playback over', async () => {
			const play = await playing();
			player.play([track('151', 60)], 0, { type: 'album', id: '9' });
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			// The account's queue, started elsewhere from the whole library, and moved here.
			await hold(['161', '162']);
			state();
			transfer(true);
			expect(await result()).toMatchObject({ status: 'applied' });
			await vi.waitFor(() => expect(player.current?.id).toBe('161'));
			await vi.waitFor(() => expect(play.mock.calls.length).toBeGreaterThanOrEqual(2));
			(play.mock.contexts.at(-1) as HTMLAudioElement).dispatchEvent(new Event('playing'));
			expect(fake.plays.filter((p) => p.end === null).map((p) => [p.trackId, p.context])).toEqual([
				['161', { type: 'library' }]
			]);
			play.mockRestore();
		});

		it('passes over the items of a library out of reach, in place', async () => {
			const play = await playing();
			player.play([track('171', 60), track('173', 60)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			queue.edit({
				type: 'add',
				source: { key: 'u', libraryId: '99', tracks: ['172'] },
				before: { type: 'manualEnd' }
			});
			await vi.waitFor(() => expect(queue.counts.manual).toBe(1));
			fake.unreached.add('99');
			const all = await itemsNow();

			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('173'));
			expect(fake.edits.filter((e) => e.type === 'jump')).toEqual([{ type: 'jump', item: all[2] }]);
			play.mockRestore();
		});

		it('does not take a track ending for a move while the next is on its way', async () => {
			fake.info = () => Promise.resolve({ variant: 'original' });
			const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
			player.play(
				['181', '182', '183'].map((id) => track(id, 60)),
				0
			);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const audio = play.mock.contexts[0] as HTMLAudioElement;
			audio.dispatchEvent(new Event('playing'));

			// The next track's audio is still on its way when the move has landed.
			fake.info = () => new Promise(() => {});
			// Before the move has landed.
			player.next();
			audio.dispatchEvent(new Event('ended'));
			await vi.waitFor(() => expect(player.current?.id).toBe('182'));
			// After it: the audio is still the first track's.
			audio.dispatchEvent(new Event('ended'));
			await settle();
			expect(player.current?.id).toBe('182');
			expect(fake.edits).toEqual([{ type: 'next' }]);
			play.mockRestore();
		});

		it('starts the context over on Repeat All when the last track is missing, once', async () => {
			const play = await playing();
			fake.catalog.set('192', track('192', 60, 'missing'));
			player.play([track('191', 60), track('192', 60, 'missing')], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			queue.edit({ type: 'repeat', mode: 'all' });
			await vi.waitFor(() => expect(queue.repeat).toBe('all'));
			fake.edits.length = 0;

			player.next();
			await vi.waitFor(() => expect(play).toHaveBeenCalledTimes(2));
			expect(player.current?.id).toBe('191');
			expect(fake.edits).toEqual([{ type: 'next' }, { type: 'next' }]);
			// A new queue keeps the repeat mode, which the next test must not get.
			queue.edit({ type: 'repeat', mode: 'off' });
			await vi.waitFor(() => expect(queue.repeat).toBe('off'));
			play.mockRestore();
		});

		it('stops on Repeat All when no track of the context can play', async () => {
			const play = await playing();
			fake.catalog.set('196', track('196', 60, 'missing'));
			fake.catalog.set('197', track('197', 60, 'missing'));
			player.play([track('195', 60), track('196', 60, 'missing'), track('197', 60, 'missing')], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			queue.edit({ type: 'repeat', mode: 'all' });
			await vi.waitFor(() => expect(queue.repeat).toBe('all'));
			fake.catalog.set('195', track('195', 60, 'missing'));
			fake.edits.length = 0;

			player.next();
			await vi.waitFor(() => expect(player.finished).toBe(true));
			await settle();
			// One lap's restart and no more.
			expect(fake.edits.filter((e) => e.type === 'next')).toHaveLength(2);
			// A new queue keeps the repeat mode, which the next test must not get.
			queue.edit({ type: 'repeat', mode: 'off' });
			await vi.waitFor(() => expect(queue.repeat).toBe('off'));
			play.mockRestore();
		});

		it('records where in the queue each play came from', async () => {
			const play = await playing();
			player.play([track('101', 60), track('102', 60)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			const audio = play.mock.contexts[0] as HTMLAudioElement;
			audio.dispatchEvent(new Event('playing'));
			queue.edit({
				type: 'add',
				source: { key: 'm', libraryId: '11', tracks: ['103'] },
				before: { type: 'manualEnd' }
			});
			await vi.waitFor(() => expect(queue.counts.manual).toBe(1));

			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('103'));
			audio.dispatchEvent(new Event('playing'));
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('102'));
			audio.dispatchEvent(new Event('playing'));
			expect(fake.plays.filter((p) => p.end === null).map((p) => [p.trackId, p.origin])).toEqual([
				['101', 'context'],
				['103', 'manual'],
				['102', 'context']
			]);
			play.mockRestore();
		});

		it('reports the queue item that plays, once the server has the queue', async () => {
			let release: (value?: unknown) => void = () => {};
			fake.holdStart = new Promise((resolve) => (release = resolve));
			player.play([track('104', 60)], 0);
			await vi.waitFor(() => expect(player.current?.id).toBe('104'));
			await settle();
			expect(queue.starting).toBe(true);
			expect(
				fake.sent.filter((m) => m.type === 'progress'),
				'it would be refused'
			).toEqual([]);

			release();
			await vi.waitFor(() => expect(queue.starting).toBe(false));
			await vi.waitFor(() =>
				expect(fake.sent.some((m) => m.report?.itemId === queue.current)).toBe(true)
			);
			expect(fake.sent.at(-1)?.report).toMatchObject({ itemId: queue.current, start: true });
		});

		it('carries out remote next and previous as queue edits', async () => {
			const play = await playing();
			player.play([track('105', 60), track('106', 60)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			fake.edits.length = 0;

			send({ type: 'execute', command: { type: 'next', commandId: crypto.randomUUID() } });
			expect(await result()).toMatchObject({ status: 'applied' });
			await vi.waitFor(() => expect(player.current?.id).toBe('106'));
			expect(fake.edits).toEqual([{ type: 'next' }]);

			fake.sent.length = 0;
			player.position = 0;
			send({ type: 'execute', command: { type: 'previous', commandId: crypto.randomUUID() } });
			expect(await result()).toMatchObject({ status: 'applied' });
			await vi.waitFor(() => expect(player.current?.id).toBe('105'));
			expect(fake.edits).toEqual([{ type: 'next' }, { type: 'previous' }]);
			play.mockRestore();
		});

		it('stops, showing nothing, when the server has no queue', async () => {
			const play = await playing();
			player.play([track('107', 60)], 0);
			await vi.waitFor(() => expect(play).toHaveBeenCalledOnce());
			// The server has none, as the dev mock does: the queue the page made is then gone.
			fake.noQueue = true;
			player.play([track('108', 60)], 0);
			await vi.waitFor(() => expect(player.current).toBeNull());
			expect(player.playing).toBe(false);
			expect(queue.queueId).toBeNull();
			play.mockRestore();
		});
	});

	// Last, since its audio goes on loading.
	it('arrives paused, where playback stopped', async () => {
		const info = vi.fn(() => Promise.resolve({ variant: 'original' }));
		fake.info = info;
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play');
		await hold(['42']);
		state({ activeDeviceId: null, status: 'stopped' });
		transfer(false);
		expect(await result()).toMatchObject({ status: 'applied' });
		expect(fake.sent[0].report).toMatchObject({
			start: true,
			itemId: items[0],
			status: 'paused',
			positionMs: 10_000
		});
		await vi.waitFor(() => expect(info).toHaveBeenCalled());
		await new Promise((resolve) => setTimeout(resolve));
		expect(play).not.toHaveBeenCalled();
	});
});
