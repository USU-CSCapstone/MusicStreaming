import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayReport, PlayerState, RealtimeServerMessage, Track } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	/** Tracks whose fetch is held back, by ID; any other arrives at once, a minute long. */
	tracks: new Map<string, Promise<unknown>>(),
	/** The audio's playback info; by default it never arrives, so nothing plays. */
	info: null as (() => Promise<unknown>) | null,
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	/** Whether messages to the server fail to go. */
	offline: false,
	sent: [] as {
		type: string;
		report: { status: string; positionMs: number; start: boolean };
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
	getTrack: (_: unknown, _library: string, id: string) =>
		fake.tracks.get(id) ?? Promise.resolve(track(id, 60)),
	getWaveform: async () => null,
	recordPlays: async (_: unknown, plays: PlayReport[]) => void fake.plays.push(...plays),
	getPlaybackInfo: () => fake.info?.() ?? new Promise(() => {})
}));

const track = (id: string, seconds: number) =>
	({
		id,
		durationUs: seconds * 1e6,
		availability: 'available',
		album: {},
		artists: []
	}) as unknown as Track;

// Before the player sets its handlers, so a test can act as the lock screen does.
const setAction = vi.spyOn(navigator.mediaSession, 'setActionHandler');
const action = (name: MediaSessionAction) =>
	setAction.mock.calls.findLast(([a]) => a === name)![1]!({ action: name });

const { player } = await import('./player.svelte');
const { controls } = await import('./controls.svelte');

const send = (message: RealtimeServerMessage) =>
	fake.listeners.forEach((listener) => listener(message));

function state(changes: Partial<PlayerState> = {}) {
	send({
		type: 'player',
		state: {
			status: 'playing',
			activeDeviceId: '2',
			itemId: '0',
			libraryId: '11',
			trackId: '41',
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

beforeEach(() => {
	fake.sent.length = 0;
	fake.plays.length = 0;
	fake.offline = false;
	fake.tracks.clear();
	fake.info = null;
	player.libraryId = '11';
	player.volume = 0.8;
});

describe('player', () => {
	it('takes playback over when another device moves it here', async () => {
		state();
		transfer(true);
		expect(await result()).toMatchObject({ status: 'applied' });
		const [report] = fake.sent;
		expect(report.type, 'taken over before answering').toBe('progress');
		expect(report.report).toMatchObject({
			start: true,
			trackId: '41',
			status: 'buffering',
			// Its own volume, not the other device's.
			volume: 0.8
		});
		expect(report.report.positionMs).toBeGreaterThanOrEqual(10_000);
		expect(player.current?.id).toBe('41');
	});

	it('says when there is nothing it can pick up', async () => {
		state({ libraryId: '12' });
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
		state({ trackId: '43' });
		transfer(true);
		state({ trackId: '43', activeDeviceId: '3' });
		arrive(track('43', 60));
		expect(await result()).toMatchObject({ status: 'failed' });
		expect(fake.sent.map((m) => m.type)).toEqual(['commandResult']);
	});

	it('starts no later than the end of the track', async () => {
		state({ trackId: '44', positionMs: 0 });
		fake.tracks.set('45', Promise.resolve(track('45', 5)));
		state({ trackId: '45', positionMs: 30_000 });
		transfer(true);
		await result();
		expect(fake.sent[0].report.positionMs).toBe(5000);
	});

	it('waits paused on a track that cannot play, rather than playing the next', async () => {
		fake.info = () => Promise.reject(new Error('gone'));
		player.tracks = [track('47', 60), track('48', 60)];
		state({ trackId: '47' });
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
		state({ status: 'paused', activeDeviceId: '1', trackId: '46' });
		// As if it had played most of the way.
		player.position = 55;
		await vi.waitFor(() => expect(player.finished).toBe(true));
		expect(fake.sent.at(-1)?.report, 'at the start of the last track').toMatchObject({
			status: 'stopped',
			positionMs: 0
		});
		state({ status: 'stopped', activeDeviceId: null, trackId: '46' });
		expect(controls.remote).toBe(false);

		void player.resume();
		expect(fake.sent.at(-1)?.report, 'loaded again, taking over').toMatchObject({
			status: 'buffering',
			positionMs: 0,
			start: true
		});
		// Its audio fails again.
		await vi.waitFor(() => expect(player.finished).toBe(true));

		state({ activeDeviceId: '2', trackId: '46' });
		expect(player.finished).toBe(false);
		state({ status: 'stopped', activeDeviceId: null, trackId: '46' });
		expect(controls.remote, 'stopped elsewhere').toBe(true);
	});

	it('shows a stop that is not its own, as one heard on reconnecting', async () => {
		fake.info = () => Promise.reject(new Error('gone'));
		player.play([track('49', 60)], 0);
		state({ status: 'paused', activeDeviceId: '1', trackId: '49' });
		await vi.waitFor(() => expect(player.finished).toBe(true));
		state({ status: 'stopped', activeDeviceId: null, trackId: '50' });
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
		state({ trackId: '56', positionMs: 30_000 });
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

	it('leaves the lock screen to the device that took over', () => {
		player.play([track('59', 60), track('60', 60)], 0);
		expect(navigator.mediaSession.metadata).not.toBeNull();
		state({ activeDeviceId: '1', trackId: '59' });
		state({ trackId: '41' });
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
		state({ activeDeviceId: null, status: 'stopped', trackId: '42' });
		transfer(false);
		expect(await result()).toMatchObject({ status: 'applied' });
		expect(fake.sent[0].report).toMatchObject({
			start: true,
			trackId: '42',
			status: 'paused',
			positionMs: 10_000
		});
		await vi.waitFor(() => expect(info).toHaveBeenCalled());
		await new Promise((resolve) => setTimeout(resolve));
		expect(play).not.toHaveBeenCalled();
	});
});
