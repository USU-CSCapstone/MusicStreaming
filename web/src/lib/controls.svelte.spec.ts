import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayerState, RealtimeServerMessage, Track } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	sent: [] as { command: { type: string } }[],
	queue: {
		queueId: 'q',
		revision: 0,
		starting: false,
		current: null,
		counts: { manual: 0, upcoming: 0, history: 0 },
		// Every item is track 41 of library 11, as the state in these tests names it.
		track: async () => ({ libraryId: '11', trackId: '41' })
	},
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
// The queue is held in a worker; here it is loaded unless a test says otherwise.
vi.mock('$lib/queue/client.svelte', () => ({ queue: fake.queue }));
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	getTrack: async (_: unknown, _library: string, id: string) => ({ id, durationUs: 60e6 }) as Track,
	getWaveform: async () => null
}));

// As reactive as the real one, so a change after rendering shows.
const queue = $state(fake.queue);
Object.assign(fake, { queue });

const { player } = await import('./player.svelte');
const { playback } = await import('./playback.svelte');
const { controls } = await import('./controls.svelte');

/** Sends the account's state with `device` at `status`, at item 0. */
function state(device: string | null, status: PlayerState['status'] = 'playing') {
	const state: PlayerState = {
		status,
		activeDeviceId: device,
		itemId: '0',
		positionMs: 10_000,
		positionAt: new Date().toISOString(),
		volume: null
	};
	fake.listeners.forEach((listener) => listener({ type: 'player', state }));
}

const sent = () => fake.sent.map((message) => message.command.type);

beforeEach(() => {
	fake.sent.length = 0;
	fake.realtime.connected = true;
	player.playing = false;
	player.libraryId = '11';
	playback.error = null;
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

	it('cannot pick up before this device has the queue', async () => {
		const { queue } = await import('$lib/queue/client.svelte');
		state(null, 'stopped');
		await vi.waitFor(() => expect(controls.pickable).toBe(true));
		const loaded = queue.queueId;
		queue.queueId = null as never;
		expect(controls.pickable).toBe(false);
		queue.queueId = loaded;
	});

	it('say so when playback could not be picked up here', async () => {
		state(null, 'stopped');
		await vi.waitFor(() => expect(controls.pickable).toBe(true));
		vi.spyOn(player, 'pickUp').mockRejectedValue(new Error('Playback moved on.'));
		controls.toggle();
		await vi.waitFor(() => expect(playback.error).toBe('Could not play that here.'));
	});

	it('leave playback stopped in another library alone', async () => {
		player.libraryId = '12';
		state(null, 'stopped');
		await vi.waitFor(() => expect(controls.track?.id).toBe('41'));
		const pickUp = vi.spyOn(player, 'pickUp').mockImplementation(async () => {});
		expect([controls.stopped, controls.pickable]).toEqual([true, false]);
		controls.toggle();
		controls.play();
		expect(pickUp).not.toHaveBeenCalled();
		expect(sent()).toEqual([]);
	});
});
