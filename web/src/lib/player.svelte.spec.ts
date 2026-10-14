import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayerState, RealtimeServerMessage, Track } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	sent: [] as { type: string; report: { status: string; positionMs: number; start: boolean } }[],
	realtime: {
		deviceId: '1',
		connected: true,
		clockOffset: 0,
		on: (listener: (message: RealtimeServerMessage) => void) => fake.listeners.push(listener),
		send(message: never) {
			fake.sent.push(message);
			return true;
		}
	}
}));

vi.mock('$lib/realtime.svelte', () => ({ realtime: fake.realtime }));
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	getWaveform: async () => null,
	// No audio arrives, so each track is skipped as one that cannot play.
	getPlaybackInfo: () => Promise.reject(new Error('gone'))
}));

const { player } = await import('./player.svelte');
const { playback } = await import('./playback.svelte');

const track = (id: string) =>
	({ id, durationUs: 60e6, availability: 'available', album: {}, artists: [] }) as unknown as Track;

function state(changes: Partial<PlayerState>) {
	const state: PlayerState = {
		status: 'playing',
		activeDeviceId: '1',
		itemId: '0',
		libraryId: '11',
		trackId: '41',
		positionMs: 0,
		positionAt: new Date().toISOString(),
		...changes
	};
	fake.listeners.forEach((listener) => listener({ type: 'player', state }));
}

beforeEach(() => {
	fake.sent.length = 0;
	player.libraryId = '11';
});

describe('player', () => {
	it('stays the player shown once it runs out of tracks, until playback starts again', async () => {
		player.play([track('41')], 0);
		// The server confirms the takeover before the track turns out to be the last.
		state({ status: 'paused' });
		// As if it had played most of the way.
		player.position = 55;
		await vi.waitFor(() => expect(player.finished).toBe(true));
		expect(fake.sent.at(-1)?.report, 'at the start of the last track').toMatchObject({
			status: 'stopped',
			positionMs: 0
		});
		state({ status: 'stopped', activeDeviceId: null });
		expect(playback.shows(player.playing || player.finished)).toBe(false);

		void player.resume();
		expect(fake.sent.at(-1)?.report, 'loaded again, taking over').toMatchObject({
			status: 'buffering',
			positionMs: 0,
			start: true
		});

		state({ activeDeviceId: '2' });
		expect(player.finished).toBe(false);
		state({ status: 'stopped', activeDeviceId: null });
		expect(playback.shows(player.playing || player.finished), 'stopped elsewhere').toBe(true);
	});

	it('shows a stop that is not its own, as one heard on reconnecting', async () => {
		player.play([track('43')], 0);
		state({ status: 'paused', trackId: '43' });
		await vi.waitFor(() => expect(player.finished).toBe(true));
		state({ status: 'stopped', activeDeviceId: null, trackId: '44' });
		expect(player.finished).toBe(false);
	});

	it('picks up only a track of its own library', () => {
		player.pickUp('12', track('42'), 5);
		expect(player.current?.id).not.toBe('42');
		expect(fake.sent).toEqual([]);
	});
});
