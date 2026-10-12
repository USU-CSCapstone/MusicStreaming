import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayerState, RealtimeServerMessage, Track } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	tracks: new Map<string, Promise<unknown>>(),
	realtime: {
		deviceId: '1',
		connected: true,
		// This browser's clock is 2 s behind the server's.
		clockOffset: 2000,
		on: (listener: (message: RealtimeServerMessage) => void) => fake.listeners.push(listener)
	}
}));

vi.mock('$lib/realtime.svelte', () => ({ realtime: fake.realtime }));
vi.mock('$lib/api/client', () => ({
	getTrack: (_: unknown, _library: string, id: string) =>
		fake.tracks.get(id) ?? new Promise(() => {}),
	getWaveform: async () => null
}));

const { playback } = await import('./playback.svelte');

const send = (message: RealtimeServerMessage) =>
	fake.listeners.forEach((listener) => listener(message));
const track = (id: string, seconds: number) => ({ id, durationUs: seconds * 1e6 }) as Track;

function state(device: string | null, trackId: string, changes: Partial<PlayerState> = {}) {
	send({
		type: 'player',
		state: {
			status: 'playing',
			activeDeviceId: device,
			itemId: '0',
			libraryId: '11',
			trackId,
			positionMs: 10_000,
			positionAt: new Date(Date.now() + 2000).toISOString(),
			...changes
		}
	});
}

beforeEach(() => {
	vi.useFakeTimers();
	fake.tracks.clear();
	fake.realtime.connected = true;
});

afterEach(() => {
	vi.useRealTimers();
});

describe('playback', () => {
	it('knows when another device holds playback, and its name', () => {
		send({ type: 'devices', devices: [{ id: '2', name: 'Desk' }] as never });
		state('1', 'a');
		expect(playback.elsewhere).toBe(false);
		state('2', 'a');
		expect(playback.elsewhere).toBe(true);
		expect(playback.deviceName).toBe('Desk');
		state('3', 'a');
		expect(playback.deviceName).toBe('another device');
		state(null, 'a', { status: 'stopped' });
		expect(playback.elsewhere).toBe(false);
	});

	it('moves the position on by the server clock while playing, up to the end', async () => {
		fake.tracks.set('b', Promise.resolve(track('b', 60)));
		// When the server said where it was, by this browser's clock.
		const at = Date.now();
		state('2', 'b');
		await vi.waitFor(() => expect(playback.track?.id).toBe('b'));
		expect(playback.positionAt(at)).toBe(10);
		expect(playback.positionAt(at + 5000)).toBe(15);
		expect(playback.positionAt(at + 120_000)).toBe(60);

		state('2', 'b', { status: 'paused' });
		expect(playback.positionAt(Date.now() + 5000)).toBe(10);

		state('2', 'b');
		fake.realtime.connected = false;
		expect(playback.positionAt(Date.now() + 5000), 'not live while offline').toBe(10);
		fake.realtime.connected = true;
		state('2', 'b', { positionAt: 'garbled' });
		expect(playback.positionAt(Date.now())).toBe(10);
	});

	it('is shown where another device plays, or where it stopped and nothing plays here', () => {
		state('2', 'e');
		expect(playback.shows(true)).toBe(true);
		state('1', 'e');
		expect(playback.shows(false)).toBe(false);
		state(null, 'e', { status: 'stopped' });
		expect(playback.shows(false)).toBe(true);
		expect(playback.shows(true)).toBe(false);
	});

	it('fetches a track again after failing to, and not for the device playing it', async () => {
		fake.tracks.set('f', Promise.reject(new Error('offline')));
		state('2', 'f');
		await Promise.resolve();
		await Promise.resolve();
		fake.tracks.set('f', Promise.resolve(track('f', 30)));
		state('2', 'f');
		await vi.waitFor(() => expect(playback.track?.id).toBe('f'));

		fake.tracks.set('g', Promise.resolve(track('g', 30)));
		state('1', 'g');
		await Promise.resolve();
		expect(playback.track?.id).toBe('f');
	});

	it('keeps the track the state names, whatever answers first', async () => {
		let slow!: (t: Track) => void;
		fake.tracks.set('c', new Promise((resolve) => (slow = resolve)));
		fake.tracks.set('d', Promise.resolve(track('d', 30)));
		state('2', 'c');
		state('2', 'd');
		await vi.waitFor(() => expect(playback.track?.id).toBe('d'));
		slow(track('c', 30));
		await Promise.resolve();
		expect(playback.track?.id).toBe('d');
	});
});
