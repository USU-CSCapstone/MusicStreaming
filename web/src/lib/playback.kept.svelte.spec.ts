import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Device, PlayerState, RealtimeServerMessage } from '$lib/api/types';

// The devices and playback as last heard, kept for the signed-in account so an app started with
// no network shows them (`last-known.ts`).

const fake = vi.hoisted(() => ({
	listeners: [] as ((message: RealtimeServerMessage) => void)[],
	realtime: {
		deviceId: null as string | null,
		connected: false,
		clockOffset: 0,
		send: () => false,
		on: (listener: (message: RealtimeServerMessage) => void) => fake.listeners.push(listener)
	}
}));

vi.mock('$lib/realtime.svelte', () => ({ realtime: fake.realtime }));
vi.mock('$lib/queue/client.svelte', () => ({
	queue: { current: null, revision: 0, rows: () => new Promise(() => {}) }
}));
vi.mock('$lib/api/client', () => ({
	getTrack: () => new Promise(() => {}),
	getWaveform: async () => null
}));

/** Delivers `message` to the playback as the server's connection would. */
const send = (message: RealtimeServerMessage) =>
	fake.listeners.forEach((listener) => listener(message));

const desk = { id: '7', name: 'Desk', type: 'desktop', connected: true } as Device;
const phone = { id: '8', name: 'Phone', type: 'phone', connected: true } as Device;
const playing: PlayerState = {
	status: 'playing',
	activeDeviceId: '8',
	itemId: 'k.1',
	positionMs: 1000,
	positionAt: new Date(0).toISOString(),
	volume: null
};

const { playback } = await import('./playback.svelte');

/**
 * Playback as a fresh page has it, with nothing heard yet: letting go of the account clears what
 * it held in memory, so only what was kept is left to show.
 */
function load() {
	playback.disconnect();
	fake.realtime.deviceId = null;
	return playback;
}

/** Hears, as a page connected to the server does, `user`'s devices and playback. */
function heard(user: string) {
	load().connect(user);
	send({ type: 'hello', deviceId: '7', serverTime: new Date().toISOString() } as never);
	send({ type: 'devices', devices: [desk, phone] });
	send({ type: 'player', state: playing });
	return playback;
}

beforeEach(() => localStorage.clear());

describe('playback kept for an app started offline', () => {
	it('a cold start offline shows the devices and the playback last heard, and this device as such', () => {
		heard('1');

		load().connect('1');
		flushSync();
		expect(playback.devices).toEqual([desk, phone]);
		expect(playback.state).toEqual(playing);
		expect(playback.thisDevice).toBe('7');
		expect(playback.elsewhere).toBe(true);
	});

	it('controlling another device offline says it is unavailable offline', async () => {
		heard('1');
		load().connect('1');
		const { connectivity } = await import('$lib/connectivity.svelte');
		connectivity.setWorkOffline(true);
		try {
			playback.pause();
			expect(playback.error).toMatch(/^Unavailable offline\. /);
		} finally {
			connectivity.setWorkOffline(false);
		}
	});

	it('another account sees nothing of them', () => {
		heard('1');

		load().connect('2');
		expect(playback.devices).toEqual([]);
		expect(playback.state).toBeNull();
		expect(playback.thisDevice).toBeNull();
	});

	it('signing out lets them go, and the account’s wipe deletes what was kept', async () => {
		heard('1').disconnect();
		expect(playback.devices).toEqual([]);
		expect(playback.state).toBeNull();

		(await import('./last-known')).forget();
		load().connect('1');
		expect(playback.devices).toEqual([]);
		expect(playback.state).toBeNull();
	});

	it('what the server says replaces what was kept', () => {
		heard('1');
		load().connect('1');
		send({ type: 'devices', devices: [desk] });
		send({ type: 'player', state: { ...playing, status: 'paused' } });
		expect(playback.devices).toEqual([desk]);
		expect(playback.state?.status).toBe('paused');
	});
});
