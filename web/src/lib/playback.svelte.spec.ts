import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayerState, RealtimeServerMessage, Track } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	tracks: new Map<string, Promise<unknown>>(),
	sent: [] as {
		type: string;
		command: { type: string; commandId: string; volume?: number; play?: boolean };
	}[],
	realtime: {
		deviceId: '1',
		connected: true,
		send(message: never) {
			if (fake.realtime.connected) fake.sent.push(message);
			return fake.realtime.connected;
		},
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
	fake.sent.length = 0;
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

	it('shows a command at once, and keeps it until the server says how it went', () => {
		state('2', 'h');
		playback.pause();
		expect(playback.shown?.status).toBe('paused');
		const id = fake.sent[0].command.commandId;
		expect(fake.sent[0].command.type).toBe('pause');

		state('2', 'h');
		expect(playback.shown?.status, 'a state from before it was carried out').toBe('paused');
		send({ type: 'commandResult', result: { commandId: id, status: 'applied' } });
		state('2', 'h', { status: 'paused' });
		state('2', 'h');
		expect(playback.shown?.status, 'the server decides from then on').toBe('playing');
	});

	it('undoes a command that failed, and says why', () => {
		state('2', 'i');
		playback.seek(20);
		expect(playback.positionAt(Date.now())).toBe(20);
		const id = fake.sent[0].command.commandId;
		const reason = 'The device playing did not answer.';
		send({ type: 'commandResult', result: { commandId: id, status: 'failed', reason } });
		expect(playback.positionAt(Date.now())).toBe(10);
		expect(playback.error).toBe(reason);
		vi.advanceTimersByTime(5000);
		expect(playback.error).toBeNull();

		fake.realtime.connected = false;
		playback.next();
		expect(playback.error).toBe('Not connected to the server.');
	});

	it('sends the volume once the slider rests', () => {
		state('2', 'j');
		for (const volume of [0.9, 0.7, 0.5]) playback.setVolume(volume);
		expect(playback.shown?.volume).toBe(0.5);
		expect(fake.sent).toEqual([]);
		vi.advanceTimersByTime(250);
		expect(fake.sent.map((m) => m.command.volume)).toEqual([0.5]);
	});

	it('lets go of what it asked once the answer can no longer come', () => {
		state('2', 'k');
		playback.pause();
		send({ type: 'hello', deviceId: '1', serverTime: new Date().toISOString() });
		state('2', 'k');
		expect(playback.shown?.status, 'reconnected').toBe('playing');

		playback.pause();
		state('3', 'k');
		expect(playback.shown?.status, 'another device took over').toBe('playing');
	});

	it('moves playback to another device, saying so until it takes over or cannot', () => {
		state('2', 'm');
		playback.transfer('3');
		expect(fake.sent[0].command).toMatchObject({ type: 'transfer', deviceId: '3', play: true });
		expect(playback.moving?.deviceId).toBe('3');
		state('2', 'm');
		expect(playback.moving?.deviceId, 'not yet').toBe('3');
		state('3', 'm');
		expect(playback.moving, 'taken over').toBeNull();

		state('3', 'm', { status: 'paused' });
		playback.transfer('4');
		expect(fake.sent[1].command, 'arrives paused').toMatchObject({ deviceId: '4', play: false });
		const commandId = fake.sent[1].command.commandId;
		const reason = 'That device is not connected.';
		send({ type: 'commandResult', result: { commandId, status: 'failed', reason } });
		expect(playback.moving).toBeNull();
		expect(playback.error).toBe(reason);

		playback.transfer('4');
		const applied = { commandId: fake.sent[2].command.commandId, status: 'applied' as const };
		send({ type: 'commandResult', result: applied });
		expect(playback.moving, 'answered before the state').toBeNull();

		fake.realtime.connected = false;
		playback.transfer('4');
		expect(playback.moving, 'not sent').toBeNull();
	});

	it('keeps the volume being dragged while a state arrives', () => {
		state('2', 'l');
		playback.setVolume(0.3);
		state('2', 'l');
		expect(playback.shown?.volume).toBe(0.3);

		fake.realtime.connected = false;
		vi.advanceTimersByTime(250);
		expect(playback.shown?.volume, 'not sent, so not shown').not.toBe(0.3);
	});
});
