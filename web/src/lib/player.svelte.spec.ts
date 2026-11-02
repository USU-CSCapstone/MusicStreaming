import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
	PlayReport,
	PlayerState,
	RealtimeServerMessage,
	SentEdit,
	Track
} from '$lib/api/types';
import type { FromWorker, RowSection, StartRequest, ToWorker } from '$lib/queue/protocol';

const fake = vi.hoisted(() => ({
	/** Makes the worker the queue client talks to; set below, before the player loads. */
	worker: null as unknown as () => Worker,
	/** Tracks whose fetch is held back, by ID; any other arrives at once, a minute long. */
	tracks: new Map<string, Promise<unknown>>(),
	/** The audio's playback info; by default it never arrives, so nothing plays. */
	info: null as (() => Promise<unknown>) | null,
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	/** Whether messages to the server fail to go. */
	offline: false,
	sent: [] as {
		type: string;
		report: {
			status: string;
			positionMs: number;
			start: boolean;
			itemId: string;
		};
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
vi.mock('$lib/queue/client.svelte', async (actual) => {
	const { QueueClient } = await actual<typeof import('$lib/queue/client.svelte')>();
	const queue = new QueueClient(() => fake.worker());
	queue.connect({ on: () => () => {} });
	return { queue };
});
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	getTrack: (_: unknown, _library: string, id: string) =>
		fake.tracks.get(id) ?? Promise.resolve(track(id, 60)),
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

/** What a fake queue's row is: a track, one still being learned, or one that cannot be. */
type Slot = string | 'unknown' | 'unavailable';

/**
 * The queue worker, as far as the player meets it: one list of items, the first `at` of them
 * History, and a row for each that is known, being learned, or unavailable. It answers a
 * start, the edits that move through the list, and windows of rows, a moment after being asked
 * as a worker does.
 */
class FakeQueue {
	slots: Slot[] = [];
	at = 0;
	/** How many plays History holds: the moves made, which a wrap back to the first item does not undo. */
	moves = 0;
	key = '';
	repeat = 'off';
	shuffled = false;
	/** Whether the item playing is said to come from the Manual Queue. */
	playingManual = false;
	revision = 0;
	/** What the page sent, in order. */
	sent: ToWorker[] = [];
	edits: SentEdit[] = [];
	starts: StartRequest[] = [];
	/** While set, a start is not answered. */
	holdStart = false;
	/** Whether the server refuses the next start. */
	refuseStart = false;
	/** Whether a Previous makes a new item, a copy, as one from History does. */
	copyOnPrevious = false;
	/** The copy that is the current item, if any. */
	copy: string | null = null;
	/** While set, the server has not confirmed the edits: the queue held is as before them. */
	holdEdits = false;
	/** The server's version, which a learned row advances. */
	version = 0;
	#heldItem: string | null = null;
	onmessage: ((event: MessageEvent<FromWorker>) => void) | null = null;
	#held: (() => void) | null = null;
	#context: unknown = null;

	postMessage(message: ToWorker) {
		this.sent.push(message);
		if (message.type === 'start') this.#start(message.id, message.request);
		else if (message.type === 'edit') this.#edit(message.edit);
		else if (message.type === 'rows') this.#rows(message.id, message.section, message.offset);
	}

	terminate() {}

	/** Tells the page, as the worker does after a change. */
	#say(message: FromWorker) {
		setTimeout(() => this.onmessage?.({ data: message } as MessageEvent<FromWorker>));
	}

	/** Tells the page what the queue is now. */
	#state() {
		this.revision++;
		const current = this.slots.length ? (this.copy ?? `${this.key}.${this.at}`) : null;
		if (!this.holdEdits) this.#heldItem = current;
		this.#say({
			type: 'state',
			summary: {
				queueId: 'q',
				context: this.#context as never,
				version: this.version,
				revision: this.revision,
				current,
				playingManual: this.playingManual,
				held: this.#heldItem,
				manual: 0,
				upNext: Math.max(0, this.slots.length - this.at - 1),
				history: this.moves,
				repeat: this.repeat as never,
				shuffled: this.shuffled
			}
		});
	}

	#start(id: number, request: StartRequest) {
		this.starts.push(request);
		const reply = () => {
			const { source, total, loaded } = request;
			this.key = source.key;
			this.#context = request.context;
			this.slots = source.tracks ?? [
				...(loaded ?? []),
				...Array<Slot>(Math.max(0, (total ?? 0) - (loaded?.length ?? 0))).fill('unknown')
			];
			this.at = request.start;
			this.moves = 0;
			this.#state();
			this.#say({ type: 'started', id, ok: true });
			if (this.refuseStart) {
				this.#say({
					type: 'refused',
					refusal: {
						code: 'validation_failed',
						message: 'The server refused this start.',
						edit: null
					}
				});
			}
		};
		if (this.holdStart) this.#held = reply;
		else reply();
	}

	/** Confirms the edits held. */
	releaseEdits() {
		this.holdEdits = false;
		this.#state();
	}

	/** Answers the held start. */
	release() {
		this.#held?.();
		this.#held = null;
	}

	#edit(edit: SentEdit) {
		this.edits.push(edit);
		if (edit.type === 'next') {
			if (this.at + 1 < this.slots.length) this.at++;
			else if (this.repeat === 'all') this.at = 0;
			this.moves++;
		} else if (edit.type === 'jump') {
			this.at = Number(edit.item.split('.')[1]);
		} else if (edit.type === 'rewind') {
			this.at = Number(edit.play.item.split('.')[1]);
		} else if (edit.type === 'previous') {
			this.at = Math.max(0, this.at - 1);
			this.copy = this.copyOnPrevious ? `${this.key}.p` : null;
			this.moves = Math.max(0, this.moves - 1);
		} else if (edit.type === 'repeat') this.repeat = edit.mode;
		else if (edit.type === 'shuffle') this.shuffled = edit.on;
		this.#state();
	}

	#rows(id: number, section: RowSection, offset: number) {
		const at = section === 'current' ? this.at : section === 'upNext' ? this.at + 1 + offset : -1;
		const slot = this.slots[at];
		const rows =
			slot === undefined
				? []
				: [
						{
							item: section === 'current' && this.copy ? this.copy : `${this.key}.${at}`,
							libraryId: '11',
							status: (slot === 'unknown' || slot === 'unavailable' ? slot : 'known') as
								| 'known'
								| 'unknown'
								| 'unavailable',
							track: slot === 'unknown' || slot === 'unavailable' ? null : slot
						}
					];
		this.#say({
			type: 'rows',
			id,
			window: { section, offset, total: rows.length, revision: this.revision, rows }
		});
	}

	/** A row learned or given up on, as when its list or page arrives or fails. */
	learn(position: number, slot: Slot) {
		this.slots[position] = slot;
		this.version++;
		this.#state();
	}

	/** Another device's move, as the queue then says it. */
	moveTo(position: number) {
		this.at = position;
		this.#state();
	}
}

const queueWorker = new FakeQueue();
fake.worker = () => queueWorker as unknown as Worker;

// Before the player sets its handlers, so a test can act as the lock screen does.
const setAction = vi.spyOn(navigator.mediaSession, 'setActionHandler');
const action = (name: MediaSessionAction) =>
	setAction.mock.calls.findLast(([a]) => a === name)![1]!({ action: name });

const { player } = await import('./player.svelte');
const { controls } = await import('./controls.svelte');
const { playback } = await import('./playback.svelte');
const { queue } = await import('$lib/queue/client.svelte');

const send = (message: RealtimeServerMessage) =>
	fake.listeners.forEach((listener) => listener(message));

/** Sends the account's state, another device playing item `0`, with `changes` over it. */
function state(changes: Partial<PlayerState> = {}) {
	send({
		type: 'player',
		state: {
			status: 'playing',
			activeDeviceId: '2',
			itemId: '0',
			positionMs: 10_000,
			positionAt: new Date().toISOString(),
			volume: 0.2,
			...changes
		}
	});
}

/** The item the player's own queue is at, as the device playing it reports it. */
const own = () => `${queueWorker.key}.${queueWorker.at}`;

/**
 * The queue holds `trackId` as its current item, and another device reports it plays it: the
 * state names the item, and the track is the queue's to say.
 */
async function heldAs(trackId: string, changes: Partial<PlayerState> = {}) {
	// Another tab plays, so this player, if it held a queue before, does not follow it to the track.
	const tab = new BroadcastChannel('jewelcase.player');
	tab.postMessage('started');
	tab.close();
	queueWorker.key = `K${trackId}`;
	queueWorker.slots = [trackId];
	queueWorker.moveTo(0);
	await settle();
	state({ itemId: `K${trackId}.0`, ...changes });
}

const transfer = (play: boolean) =>
	send({
		type: 'execute',
		command: { type: 'transfer', commandId: crypto.randomUUID(), deviceId: '1', play }
	});

const result = () => vi.waitFor(() => fake.sent.find((m) => m.type === 'commandResult')!.result);

beforeEach(() => {
	queueWorker.sent.length = queueWorker.edits.length = queueWorker.starts.length = 0;
	queueWorker.holdStart = false;
	queueWorker.copyOnPrevious = queueWorker.holdEdits = false;
	queueWorker.copy = null;
	queueWorker.refuseStart = false;
	queueWorker.repeat = 'off';
	queueWorker.shuffled = false;
	queueWorker.playingManual = false;
	queueWorker.release();
	fake.sent.length = 0;
	fake.plays.length = 0;
	fake.offline = false;
	fake.tracks.clear();
	fake.info = null;
	player.libraryId = '11';
	player.volume = 0.8;
	player.finished = false;
});

describe('player', () => {
	it('takes playback over when another device moves it here', async () => {
		await heldAs('41');
		transfer(true);
		expect(await result()).toMatchObject({ status: 'applied' });
		const [report] = fake.sent;
		expect(report.type, 'taken over before answering').toBe('progress');
		expect(report.report).toMatchObject({
			start: true,
			itemId: 'K41.0',
			status: 'buffering',
			// Its own volume, not the other device's.
			volume: 0.8
		});
		expect(report.report.positionMs).toBeGreaterThanOrEqual(10_000);
		expect(player.current?.id).toBe('41');
	});

	it('says when there is nothing it can pick up', async () => {
		await heldAs('41');
		player.libraryId = '12';
		transfer(true);
		expect(await result()).toMatchObject({
			status: 'failed',
			reason: 'This device could not pick up playback.'
		});
		expect(fake.sent.map((m) => m.type)).toEqual(['commandResult']);
	});

	it('does not take back what another device took over meanwhile', async () => {
		let arrive: (track: Track) => void = () => {};
		fake.tracks.set('43', new Promise((resolve) => (arrive = resolve)));
		await heldAs('43');
		transfer(true);
		state({ itemId: 'K43.0', activeDeviceId: '3' });
		arrive(track('43', 60));
		expect(await result()).toMatchObject({ status: 'failed' });
		expect(fake.sent.map((m) => m.type)).toEqual(['commandResult']);
	});

	it('starts no later than the end of the track', async () => {
		await heldAs('44', { positionMs: 0 });
		fake.tracks.set('45', Promise.resolve(track('45', 5)));
		await heldAs('45', { positionMs: 30_000 });
		transfer(true);
		await result();
		expect(fake.sent[0].report.positionMs).toBe(5000);
	});

	it('waits paused on a track that cannot play, rather than playing the next', async () => {
		fake.info = () => Promise.reject(new Error('gone'));
		await heldAs('47');
		transfer(false);
		await result();
		// For the failure to be handled.
		await new Promise((resolve) => setTimeout(resolve, 50));
		expect(player.current?.id).toBe('47');
	});

	it('stays the player shown once it runs out of tracks, until playback starts again', async () => {
		fake.info = () => Promise.reject(new Error('gone'));
		player.play([track('46', 60)], 0);
		// The server confirms the takeover before the track turns out to be the last.
		state({ status: 'paused', activeDeviceId: '1', itemId: own() });
		// As if it had played most of the way.
		player.position = 55;
		await vi.waitFor(() => expect(player.finished).toBe(true));
		expect(fake.sent.at(-1)?.report, 'at the start of the last track').toMatchObject({
			status: 'stopped',
			positionMs: 0
		});
		state({ status: 'stopped', activeDeviceId: null, itemId: own() });
		expect(controls.remote).toBe(false);

		void player.resume();
		expect(fake.sent.at(-1)?.report, 'loaded again, taking over').toMatchObject({
			status: 'buffering',
			positionMs: 0,
			start: true
		});
		// Its audio fails again.
		await vi.waitFor(() => expect(player.finished).toBe(true));

		state({ activeDeviceId: '2', itemId: own() });
		expect(player.finished).toBe(false);
		state({ status: 'stopped', activeDeviceId: null, itemId: own() });
		expect(controls.remote, 'stopped elsewhere').toBe(true);
	});

	it('shows a stop that is not its own, as one heard on reconnecting', async () => {
		fake.info = () => Promise.reject(new Error('gone'));
		player.play([track('49', 60)], 0);
		state({ status: 'paused', activeDeviceId: '1', itemId: own() });
		await vi.waitFor(() => expect(player.finished).toBe(true));
		state({ status: 'stopped', activeDeviceId: null, itemId: 'other.0' });
		expect(player.finished).toBe(false);
	});

	it('stays paused when paused before the track has its audio', async () => {
		let arrive: (info: unknown) => void = () => {};
		fake.info = () => new Promise((resolve) => (arrive = resolve));
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		player.play([track('51', 60)], 0);
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
		await heldAs('56', { positionMs: 30_000 });
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

	it('records a play of a Manual Queue item as manual', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		queueWorker.playingManual = true;
		player.play([track('61', 60)], 0);
		await vi.waitFor(() => expect(play).toHaveBeenCalled());
		expect(queue.playingManual).toBe(true);
		const audio = play.mock.contexts[0] as HTMLAudioElement;
		audio.dispatchEvent(new Event('playing'));
		expect(fake.plays.filter((p) => p.trackId === '61').map((p) => p.origin)).toEqual(['manual']);
		play.mockRestore();
	});

	it('leaves the lock screen to the device that took over', async () => {
		player.play([track('59', 60), track('60', 60)], 0);
		// Once the server has the queue, it hears of this device.
		await settle();
		expect(navigator.mediaSession.metadata).not.toBeNull();
		state({ activeDeviceId: '1', itemId: own() });
		state({ itemId: 'other.0' });
		expect(player.playing).toBe(false);
		expect(navigator.mediaSession.metadata).toBeNull();

		// As a headset does as it connects, which must not take playback back here.
		fake.sent.length = 0;
		action('play');
		action('nexttrack');
		expect(fake.sent).toEqual([]);
		expect(player.current?.id).toBe('59');
	});

	// Last, since its audio goes on loading.
	it('arrives paused, where playback stopped', async () => {
		const info = vi.fn(() => Promise.resolve({ variant: 'original' }));
		fake.info = info;
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play');
		await heldAs('42', { activeDeviceId: null, status: 'stopped' });
		transfer(false);
		expect(await result()).toMatchObject({ status: 'applied' });
		expect(fake.sent[0].report).toMatchObject({
			start: true,
			itemId: 'K42.0',
			status: 'paused',
			positionMs: 10_000
		});
		await vi.waitFor(() => expect(info).toHaveBeenCalled());
		await new Promise((resolve) => setTimeout(resolve));
		expect(play).not.toHaveBeenCalled();
	});
});

/** Lets the queue worker's answers, and what the player does with them, happen. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 30));

describe('player and the queue', () => {
	const album = { type: 'album', id: '9' } as const;
	const recipe = { type: 'album', albumId: '9', sort: 'album', order: 'asc' } as const;
	/** Plays a started queue's audio, and answers with the element so a test can end its track. */
	async function playing(...ids: string[]) {
		fake.info = () => Promise.resolve({ variant: 'original' });
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		player.play(
			ids.map((id) => track(id, 60) as never),
			0,
			album
		);
		await settle();
		return { play, audio: play.mock.contexts[0] as HTMLAudioElement };
	}

	it('start plays immediately, and reports its item once the server has the queue', async () => {
		queueWorker.holdStart = true;
		player.play([track('61', 60), track('62', 60), track('63', 60)] as never, 1, album);

		expect(player.current?.id).toBe('62');
		expect(player.playing).toBe(true);
		const [request] = queueWorker.starts;
		expect(request).toMatchObject({
			start: 1,
			context: album,
			source: { libraryId: '11', tracks: ['61', '62', '63'] }
		});
		// The server refuses an item its queue lacks, and would not take the report again.
		expect(fake.sent, 'nothing reported while the start is on its way').toEqual([]);

		queueWorker.release();
		await settle();
		expect(fake.sent.at(-1)!.report).toMatchObject({
			start: true,
			itemId: `${request.source.key}.1`,
			status: 'buffering'
		});
	});

	it('reports a Previous that makes a new item once the server has the edit', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		player.play([track('64', 60), track('65', 60), track('67', 60)] as never, 0, album);
		await settle();
		player.next();
		await settle();
		fake.sent.length = 0;
		queueWorker.copyOnPrevious = true;
		queueWorker.holdEdits = true;
		player.position = 0;
		player.previous();
		await settle();
		const named = () => fake.sent.filter((m) => m.report.itemId === `${queueWorker.key}.p`);
		expect(named(), "the copy is not the server's yet").toEqual([]);
		queueWorker.releaseEdits();
		await settle();
		expect(fake.sent.at(-1)!.report).toMatchObject({ itemId: `${queueWorker.key}.p` });
	});

	it('reports again when the queue reaches the item its report was refused for', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		player.play([track('66', 60)] as never, 0, album);
		await settle();
		const item = own();
		// The server answered with a state that does not have this device at the item.
		state({ activeDeviceId: null, status: 'stopped', itemId: 'older.0' });
		fake.sent.length = 0;
		queueWorker.learn(0, '66');
		await settle();
		expect(fake.sent.at(-1)?.report).toMatchObject({ itemId: item });
	});

	it("sends a view's recipe with the clicked row and the rows it loaded, and no tracks", () => {
		queueWorker.holdStart = true;
		player.play([track('61', 60), track('62', 60)] as never, 1, album, { recipe, total: 5 });

		const [request] = queueWorker.starts;
		expect(request).toMatchObject({ start: 1, startTrackId: '62', total: 5, loaded: ['61', '62'] });
		expect(request.source).toMatchObject({ libraryId: '11', recipe });
		expect(request.source).not.toHaveProperty('tracks');
	});

	it('moves on with a next edit, applied at once, and plays what the queue moves to', async () => {
		await playing('64', '65', '66');
		player.next();

		expect(queueWorker.edits).toEqual([{ type: 'next' }]);
		await vi.waitFor(() => expect(player.current?.id).toBe('65'));
		expect(fake.sent.at(-1)!.report.itemId).toBe(`${queueWorker.key}.1`);
	});

	it('moves on with a next edit when the track ends, or plays it again under Repeat One', async () => {
		const { play, audio } = await playing('64', '65');
		audio.dispatchEvent(new Event('ended'));
		expect(queueWorker.edits).toEqual([{ type: 'next' }]);
		await vi.waitFor(() => expect(player.current?.id).toBe('65'));

		player.setRepeat('one');
		await settle();
		const plays = play.mock.calls.length;
		audio.dispatchEvent(new Event('ended'));
		expect(queueWorker.edits.filter((e) => e.type === 'next')).toHaveLength(1);
		await vi.waitFor(() => expect(play.mock.calls.length).toBeGreaterThan(plays));
		expect(player.current?.id).toBe('65');
	});

	it('unknown rows wait or skip: waits for a row that is being learned, and plays it once it is', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		player.play([track('71', 60), track('72', 60)] as never, 1, album, { recipe, total: 4 });
		await settle();
		player.next();
		await settle();
		expect(queueWorker.edits).toEqual([{ type: 'next' }]);
		expect(player.current?.id, 'still on the track before').toBe('72');

		queueWorker.learn(2, '73');
		await vi.waitFor(() => expect(player.current?.id).toBe('73'));
		expect(queueWorker.edits, 'nothing skipped').toHaveLength(1);
	});

	it('unknown rows wait or skip: skips in place a row that cannot be learned, and stops at the end of the queue', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		player.play([track('71', 60), track('72', 60)] as never, 1, album, { recipe, total: 5 });
		await settle();
		queueWorker.learn(2, 'unavailable');
		queueWorker.learn(3, '74');
		queueWorker.learn(4, 'unavailable');
		player.next();
		await vi.waitFor(() => expect(player.current?.id).toBe('74'));
		expect(queueWorker.edits).toEqual([{ type: 'next' }, { type: 'next' }]);

		player.next();
		await vi.waitFor(() => expect(player.finished).toBe(true));
	});

	it('skips in place a track that is missing, in the way it was going', async () => {
		fake.tracks.set('82', Promise.resolve(track('82', 60, 'missing')));
		fake.info = () => Promise.resolve({ variant: 'original' });
		player.play([track('81', 60), track('82', 60), track('83', 60)] as never, 0, album);
		await settle();
		player.next();
		await vi.waitFor(() => expect(player.current?.id).toBe('83'));
		expect(queueWorker.edits).toEqual([{ type: 'next' }, { type: 'next' }]);

		queueWorker.learn(1, '82');
		player.position = 0;
		player.previous();
		await vi.waitFor(() => expect(player.current?.id).toBe('81'));
		expect(queueWorker.edits.map((e) => e.type)).toEqual(['next', 'next', 'previous', 'previous']);
		expect(queueWorker.edits[2]).toMatchObject({ key: expect.stringMatching(/^[0-9a-f-]{36}$/) });
	});

	describe('Previous', () => {
		it('restarts a track that is under way, and one with nothing played before it', async () => {
			await playing('84', '85');
			player.position = 10;
			player.previous();
			expect(player.position).toBe(0);
			player.previous();
			expect(queueWorker.edits, 'nothing is before the first').toEqual([]);
		});

		it('steps back, with a key for the copy it may make, near the start of a track', async () => {
			await playing('84', '85');
			player.next();
			await vi.waitFor(() => expect(player.current?.id).toBe('85'));
			player.position = 1;
			player.previous();

			expect(queueWorker.edits.at(-1)).toMatchObject({ type: 'previous', key: expect.any(String) });
			await vi.waitFor(() => expect(player.current?.id).toBe('84'));
		});
	});

	it('follows the queue when another device moves it, while this one plays', async () => {
		await playing('86', '87', '88');
		queueWorker.moveTo(2);
		await vi.waitFor(() => expect(player.current?.id).toBe('88'));
		expect(queueWorker.edits).toEqual([]);
	});

	it('jumps to an item of Up Next with a jump edit, and plays it', async () => {
		await playing('86', '87', '88');
		const [item] = [`${queueWorker.key}.2`];
		queueWorker.edits.length = 0;
		player.jump(item);

		expect(queueWorker.edits).toEqual([{ type: 'jump', item }]);
		await vi.waitFor(() => expect(player.current?.id).toBe('88'));
	});

	it('rewinds to a play of History with a rewind edit under a key of its own', async () => {
		await playing('86', '87', '88');
		queueWorker.edits.length = 0;
		player.rewind({ play: 4, item: `${queueWorker.key}.0` });

		expect(queueWorker.edits).toEqual([
			{
				type: 'rewind',
				play: { play: 4, item: `${queueWorker.key}.0` },
				key: expect.stringMatching(/^[0-9a-f-]{36}$/)
			}
		]);
	});

	it('takes playback over when it rewinds, though another device holds it', async () => {
		await playing('86', '87', '88');
		queueWorker.moveTo(2);
		await vi.waitFor(() => expect(player.current?.id).toBe('88'));
		// Another device takes playback, so this one only shows the queue.
		state({ itemId: own(), activeDeviceId: '1' });
		state({ itemId: own(), activeDeviceId: '3' });
		await settle();
		expect(player.playing).toBe(false);
		fake.sent.length = 0;

		player.rewind({ play: 0, item: `${queueWorker.key}.0` });
		await vi.waitFor(() => expect(player.current?.id).toBe('86'));
		expect(fake.sent.some((m) => m.type === 'progress' && m.report.start)).toBe(true);
	});

	it('loads the rewound item afresh even when it is another copy of the track playing', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		player.play([track('87', 60), track('87', 60), track('88', 60)] as never, 1, album);
		await settle();
		queueWorker.moves = 1;
		const before = play.mock.calls.length;

		player.rewind({ play: 0, item: `${queueWorker.key}.0` });
		await settle();
		expect(play.mock.calls.length).toBeGreaterThan(before);
	});

	it('steps back over a track that cannot play after a rewind, not forward', async () => {
		await playing('86', '87', '88');
		queueWorker.slots[1] = 'unavailable';
		queueWorker.moves = 2;
		queueWorker.moveTo(2);
		await vi.waitFor(() => expect(player.current?.id).toBe('88'));
		player.rewind({ play: 0, item: `${queueWorker.key}.1` });
		await vi.waitFor(() => expect(player.current?.id).toBe('86'));
	});

	it('moves on once the start is answered, if Next was pressed before', async () => {
		queueWorker.holdStart = true;
		player.play([track('61', 60), track('62', 60)] as never, 0, album);
		player.next();
		expect(queueWorker.edits).toEqual([]);
		queueWorker.release();
		await vi.waitFor(() => expect(player.current?.id).toBe('62'));
		expect(queueWorker.edits).toEqual([{ type: 'next' }]);
	});

	it('sends shuffle with a seed of its own, and repeat, as edits', () => {
		player.setShuffle(true);
		player.setRepeat('all');

		const [shuffle, repeat] = queueWorker.edits;
		expect(shuffle).toMatchObject({ type: 'shuffle', on: true });
		const { seed } = shuffle as { seed: number };
		expect(Number.isInteger(seed) && seed >= 0 && seed < 2 ** 32).toBe(true);
		expect(repeat).toEqual({ type: 'repeat', mode: 'all' });
	});

	it("records each play with the queue's context", async () => {
		const { audio } = await playing('89', '90');
		audio.dispatchEvent(new Event('playing'));

		expect(fake.plays.at(-1)).toMatchObject({ trackId: '89', context: album, origin: 'context' });
	});

	describe('when tracks cannot play', () => {
		it('stops after one lap of a queue none of which can play, even under Repeat All', async () => {
			fake.info = () => Promise.reject(new Error('gone'));
			queueWorker.repeat = 'all';
			player.play([track('92', 60), track('93', 60)] as never, 0, album);
			await vi.waitFor(() => expect(player.finished).toBe(true));
			await settle();

			// Each track tried once, and the first again as the lap comes round.
			expect(queueWorker.edits).toEqual([{ type: 'next' }, { type: 'next' }]);
			expect(playback.error).toBe('None of the songs in the queue can play.');
		});

		it('tries every track before stopping, though it started in the middle, under Repeat All', async () => {
			fake.info = () => Promise.reject(new Error('gone'));
			queueWorker.repeat = 'all';
			player.play([track('92', 60), track('93', 60), track('94', 60)] as never, 1, album);
			await vi.waitFor(() => expect(player.finished).toBe(true));
			await settle();

			expect(queueWorker.edits).toHaveLength(3);
		});

		it('stops after one lap when the audio itself fails, under Repeat All', async () => {
			fake.info = () => Promise.resolve({ variant: 'original' });
			const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
			queueWorker.repeat = 'all';
			player.play([track('92', 60), track('93', 60)] as never, 0, album);
			await settle();
			const audio = play.mock.contexts[0] as HTMLAudioElement;
			// As if it were playing, which the mocked `play` leaves it not.
			Object.defineProperty(audio, 'paused', { value: false, configurable: true });
			for (let i = 0; i < 6 && !player.finished; i++) {
				audio.dispatchEvent(new Event('error'));
				await settle();
			}

			// The element is the player's for good: it is left as it was.
			delete (audio as { paused?: boolean }).paused;
			expect(player.finished).toBe(true);
			expect(queueWorker.edits.length).toBeLessThanOrEqual(2);
		});

		it('moves on in the new queue when the track fails before the start is answered', async () => {
			queueWorker.holdStart = true;
			fake.info = () => Promise.reject(new Error('gone'));
			player.play([track('92', 60), track('93', 60)] as never, 0, album);
			await settle();
			expect(queueWorker.edits).toEqual([]);
			queueWorker.release();

			await vi.waitFor(() => expect(queueWorker.edits).toEqual([{ type: 'next' }]));
		});

		it('does not ask again for a track once it is paused', async () => {
			vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
			try {
				let asked = 0;
				fake.info = () => {
					asked++;
					return Promise.reject(new TypeError('Failed to fetch'));
				};
				player.play([track('94', 60)] as never, 0, album);
				await vi.advanceTimersByTimeAsync(100);
				player.pause();
				const before = asked;
				await vi.advanceTimersByTimeAsync(10_000);

				expect(asked).toBe(before);
			} finally {
				vi.useRealTimers();
			}
		});

		it('waits and asks again when the server cannot be reached, and never skips', async () => {
			vi.useFakeTimers({ toFake: ['setTimeout'] });
			try {
				let asked = 0;
				fake.info = () => {
					asked++;
					return Promise.reject(new TypeError('Failed to fetch'));
				};
				player.play([track('94', 60), track('95', 60)] as never, 0, album);
				await vi.advanceTimersByTimeAsync(10_000);

				expect(asked).toBeGreaterThan(1);
				expect(queueWorker.edits).toEqual([]);
				expect(player.current?.id).toBe('94');
			} finally {
				vi.useRealTimers();
			}
		});
	});

	describe('taking over', () => {
		/** A queue of three here, `at` playing, which the other device reports it plays. */
		async function queued(slot: string) {
			queueWorker.key = 'K';
			queueWorker.slots = ['91', slot, '93'];
			queueWorker.moveTo(1);
			await settle();
		}

		it('plays the queue’s own current item, without starting a queue', async () => {
			await queued('92');
			state({ itemId: 'K.1' });
			transfer(true);

			expect(await result()).toMatchObject({ status: 'applied' });
			expect(queueWorker.starts).toEqual([]);
			expect(fake.sent[0].report).toMatchObject({ itemId: 'K.1' });
		});

		it('fails, and starts no queue, while the item’s row is still being learned', async () => {
			await queued('unknown');
			state({ itemId: 'K.1' });
			transfer(true);

			expect(await result()).toMatchObject({ status: 'failed' });
			expect(queueWorker.starts).toEqual([]);
		});

		it('fails, and starts no queue, for a place that is not an item of a queue', async () => {
			await queued('92');
			state({ itemId: '0' });
			transfer(true);

			expect(await result()).toMatchObject({ status: 'failed' });
			expect(queueWorker.starts).toEqual([]);
		});

		it('fails, and starts no queue, before the queue is here at all', async () => {
			queueWorker.key = 'L';
			queueWorker.slots = ['91'];
			queueWorker.moveTo(0);
			await settle();
			state({ itemId: 'K.1' });
			transfer(true);

			expect(await result()).toMatchObject({ status: 'failed' });
			expect(queueWorker.starts).toEqual([]);
		});
	});

	it('plays the first track again when the last ends under Repeat All', async () => {
		const { audio } = await playing('96', '97');
		player.setRepeat('all');
		player.next();
		await vi.waitFor(() => expect(player.current?.id).toBe('97'));
		audio.dispatchEvent(new Event('ended'));

		await vi.waitFor(() => expect(player.current?.id).toBe('96'));
		expect(queueWorker.edits.map((e) => e.type)).toEqual(['repeat', 'next', 'next']);
	});

	it('moves on once, not twice, when the track ends while the next row is being learned', async () => {
		fake.info = () => Promise.resolve({ variant: 'original' });
		const play = vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
		player.play([track('71', 60), track('72', 60)] as never, 1, album, { recipe, total: 4 });
		await settle();
		player.next();
		await settle();
		(play.mock.contexts[0] as HTMLAudioElement).dispatchEvent(new Event('ended'));
		await settle();

		expect(queueWorker.edits).toEqual([{ type: 'next' }]);
	});

	it('keeps the track clicked while the queue it replaces is heard, until the start is answered', async () => {
		queueWorker.key = 'old';
		queueWorker.slots = ['881', '882'];
		queueWorker.holdStart = true;
		player.play([track('61', 60), track('62', 60)] as never, 0, album);
		queueWorker.moveTo(0);
		await settle();

		expect(player.current?.id).toBe('61');
	});

	it('goes on with the same track when the server’s queue made it another item', async () => {
		let asked = 0;
		fake.info = () => {
			asked++;
			return Promise.resolve({ variant: 'original' });
		};
		player.play([track('61', 60), track('62', 60)] as never, 0, album, { recipe, total: 3 });
		await settle();
		queueWorker.slots = ['60', '61', '62'];
		queueWorker.moveTo(1);
		await settle();

		expect(asked).toBe(1);
		expect(fake.sent.at(-1)!.report.itemId).toBe(`${queueWorker.key}.1`);
	});

	it('pauses on the track chosen, and says so, when the server refuses the start', async () => {
		queueWorker.refuseStart = true;
		player.play([track('61', 60), track('62', 60)] as never, 0, album);
		await settle();

		expect(player.playing).toBe(false);
		expect(player.current?.id).toBe('61');
		expect(playback.error).toBe('The server refused this start.');
	});
});
