import { page, userEvent } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Device, PlayerState } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	realtime: { deviceId: '2', connected: true, offline: false },
	playback: { devices: [] as unknown[], state: null as unknown, track: null as unknown },
	controls: {
		playing: true,
		remote: true,
		track: { id: '41', title: 'Brain Stew' } as unknown,
		pickable: true,
		playHere: () => {}
	}
}));

vi.mock('$lib/realtime.svelte', () => ({ realtime: fake.realtime }));
vi.mock('$lib/playback.svelte', () => ({ playback: fake.playback }));
vi.mock('$lib/controls.svelte', () => ({ controls: fake.controls }));

const { panels } = await import('$lib/panels.svelte');
const { default: DevicesPanel } = await import('./DevicesPanel.svelte');

const device = (id: string, name: string, type: Device['type'], connected: boolean) =>
	({ id, name, type, connected, firstSeenAt: '', lastSeenAt: '' }) as Device;

const playingOn = (activeDeviceId: string): PlayerState => ({
	status: 'playing',
	activeDeviceId,
	itemId: '0',
	trackId: '41',
	positionMs: 0,
	positionAt: new Date().toISOString()
});

beforeEach(() => {
	Object.assign(fake.realtime, { connected: true, offline: false });
	fake.playback.devices = [
		device('3', 'Old Laptop', 'desktop', false),
		device('1', 'Desk', 'desktop', true),
		device('2', 'Phone', 'phone', true)
	];
	fake.playback.state = playingOn('1');
	fake.playback.track = fake.controls.track;
	fake.controls.playing = true;
	fake.controls.remote = true;
	fake.controls.pickable = true;
	panels.devices = true;
});

const rows = () => page.getByRole('list', { name: 'Devices' }).getByRole('listitem');

describe('DevicesPanel', () => {
	it('lists this device first, then those connected, then the rest, saying which', async () => {
		render(DevicesPanel);
		await expect.element(rows().nth(0)).toHaveTextContent(/Phone\s*This device · Connected/);
		await expect.element(rows().nth(1)).toHaveTextContent(/Desk\s*Playing · Brain Stew/);
		await expect.element(rows().nth(2)).toHaveTextContent(/Old Laptop\s*Not connected/);
		await expect.element(rows().nth(2)).toHaveClass('off');
	});

	it("takes playback elsewhere here from this device's row", async () => {
		const playHere = vi.spyOn(fake.controls, 'playHere');
		render(DevicesPanel);
		const here = rows().nth(0).getByRole('button', { name: 'Play here' });
		await here.click();
		expect(playHere).toHaveBeenCalledOnce();
		await expect.element(rows().nth(1).getByRole('button')).not.toBeInTheDocument();
	});

	it('offers nothing to take of a library this device does not play from', async () => {
		fake.controls.pickable = false;
		render(DevicesPanel);
		await expect.element(rows().nth(0)).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Play here' })).not.toBeInTheDocument();
	});

	it('offers nothing to take while this device plays, or is offline', async () => {
		fake.controls.remote = false;
		render(DevicesPanel);
		await expect.element(rows().nth(0)).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Play here' })).not.toBeInTheDocument();
	});

	it('shows a pause as the player bar does', async () => {
		fake.controls.playing = false;
		render(DevicesPanel);
		await expect.element(rows().nth(1)).toHaveTextContent(/Paused · Brain Stew/);
	});

	it('shows nothing as current while offline', async () => {
		Object.assign(fake.realtime, { connected: false, offline: true });
		render(DevicesPanel);
		await expect.element(page.getByText('Offline, so this may be out of date')).toBeVisible();
		await expect.element(rows().nth(0)).toHaveTextContent(/Phone\s*This device · Not connected/);
		await expect.element(rows().nth(1)).toHaveTextContent(/Desk\s*Was playing · Brain Stew/);
		// Taken here from where it was last heard of.
		await expect.element(rows().nth(0).getByRole('button', { name: 'Play here' })).toBeVisible();
	});

	it('says when there is nothing yet, and closes with Escape', async () => {
		fake.playback.devices = [];
		fake.realtime.connected = false;
		render(DevicesPanel);
		await expect.element(page.getByText('Your devices show once connected.')).toBeVisible();
		await userEvent.keyboard('{Escape}');
		expect(panels.devices).toBe(false);
	});
});
