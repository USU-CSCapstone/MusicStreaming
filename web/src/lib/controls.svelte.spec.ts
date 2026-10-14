import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayerState, RealtimeServerMessage, Track } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	sent: [] as { command: { type: string } }[],
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
	getTrack: async (_: unknown, _library: string, id: string) => ({ id, durationUs: 60e6 }) as Track,
	getWaveform: async () => null
}));

const { player } = await import('./player.svelte');
const { controls } = await import('./controls.svelte');

function state(device: string | null, status: PlayerState['status'] = 'playing') {
	const state: PlayerState = {
		status,
		activeDeviceId: device,
		itemId: '0',
		libraryId: '11',
		trackId: '41',
		positionMs: 10_000,
		positionAt: new Date().toISOString()
	};
	fake.listeners.forEach((listener) => listener({ type: 'player', state }));
}

const sent = () => fake.sent.map((message) => message.command.type);

beforeEach(() => {
	fake.sent.length = 0;
	fake.realtime.connected = true;
	player.playing = false;
	vi.restoreAllMocks();
});

describe('controls', () => {
	it('control this device while nothing plays elsewhere', () => {
		state('1');
		const toggle = vi.spyOn(player, 'toggle').mockImplementation(() => {});
		const next = vi.spyOn(player, 'next').mockImplementation(() => {});
		controls.toggle();
		controls.next();
		expect(controls.remote).toBe(false);
		expect([toggle.mock.calls.length, next.mock.calls.length]).toEqual([1, 1]);
		expect(sent()).toEqual([]);
	});

	it('control the device playing elsewhere', () => {
		state('2');
		expect(controls.remote).toBe(true);
		controls.toggle();
		controls.next();
		controls.seek(30);
		controls.play();
		expect(sent()).toEqual(['pause', 'next', 'seek', 'play']);
	});

	it.each([true, false])(
		'control this device once it plays, before the server hears (connected: %s)',
		(connected) => {
			state('2');
			fake.realtime.connected = connected;
			player.playing = true;
			const toggle = vi.spyOn(player, 'toggle').mockImplementation(() => {});
			controls.toggle();
			expect(controls.remote).toBe(false);
			expect(toggle).toHaveBeenCalledOnce();
			expect(sent()).toEqual([]);
			state(null, 'stopped');
			expect(controls.remote, 'nor where playback stopped').toBe(false);
		}
	);

	it('pick up here where playback stopped elsewhere, and do nothing else', async () => {
		state(null, 'stopped');
		await vi.waitFor(() => expect(controls.track?.id).toBe('41'));
		const pickUp = vi.spyOn(player, 'pickUp').mockImplementation(async () => {});
		expect(controls.stopped).toBe(true);
		controls.next();
		controls.seek(5);
		controls.toggle();
		expect(sent()).toEqual([]);
		expect(pickUp).toHaveBeenCalledOnce();
	});
});
