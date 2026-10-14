import { page, userEvent } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Device, PlayerState } from '$lib/api/types';

const fake = vi.hoisted(() => ({
	realtime: { deviceId: '2', connected: true, offline: false },
	playback: {
		devices: [] as unknown[],
		state: null as unknown,
		track: null as unknown,
		moving: null as { deviceId: string; commandId: string } | null,
		transfer: (() => {}) as (deviceId: string) => void
	},
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

// As reactive as the real ones, so a change after rendering shows.
const playback = $state(fake.playback);
const controls = $state(fake.controls);
Object.assign(fake, { playback, controls });

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
	vi.restoreAllMocks();
	Object.assign(fake.realtime, { connected: true, offline: false });
	fake.playback.devices = [
		device('3', 'Old Laptop', 'desktop', false),
		device('1', 'Desk', 'desktop', true),
		device('2', 'Phone', 'phone', true)
	];
	fake.playback.state = playingOn('1');
	fake.playback.track = fake.controls.track;
	fake.playback.moving = null;
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

		// Once it plays here the button goes, leaving focus on the list, and says so.
		fake.controls.remote = false;
		fake.playback.state = playingOn('2');
		await expect.element(here).not.toBeInTheDocument();
		await expect.element(page.getByRole('list', { name: 'Devices' })).toHaveFocus();
		await expect.element(page.getByRole('status')).toHaveTextContent('Playback is on this device');
	});

	it('leaves focus where it is when a button it never had goes', async () => {
		render(DevicesPanel);
		const here = rows().nth(0).getByRole('button', { name: 'Play here' });
		await expect.element(here).toBeVisible();
		(document.activeElement as HTMLElement).blur();

		// Playback starts here from elsewhere, as another device moving it here would.
		fake.controls.remote = false;
		fake.playback.state = playingOn('2');
		await expect.element(here).not.toBeInTheDocument();
		expect(document.activeElement).toBe(document.body);
	});

	it('offers nothing to take of a library this device does not play from', async () => {
		fake.controls.pickable = false;
		render(DevicesPanel);
		await expect.element(rows().nth(0)).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Play here' })).not.toBeInTheDocument();
	});

	it('moves playback to another connected device', async () => {
		fake.playback.devices.push(device('4', 'Living Room', 'tv', true));
		const transfer = vi.spyOn(fake.playback, 'transfer');
		render(DevicesPanel);
		const room = rows().nth(2);
		await expect.element(room).toHaveTextContent(/Living Room\s*Connected/);
		const there = room.getByRole('button', { name: 'Play there, on Living Room' });
		await there.click();
		expect(transfer).toHaveBeenCalledWith('4');
		// Not to the device playing it, nor one not connected.
		expect(page.getByRole('button', { name: 'Play there' }).elements()).toHaveLength(1);

		// Once it plays there the button goes, leaving focus on the list, and says so.
		fake.playback.state = playingOn('4');
		await expect.element(there).not.toBeInTheDocument();
		await expect.element(page.getByRole('list', { name: 'Devices' })).toHaveFocus();
		await expect.element(page.getByRole('status')).toHaveTextContent('Playback is on Living Room');
	});

	it('says playback is on its way, and moves it one place at a time', async () => {
		fake.playback.devices.push(device('4', 'Living Room', 'tv', true));
		fake.playback.moving = { deviceId: '4', commandId: 'c' };
		const transfer = vi.spyOn(fake.playback, 'transfer');
		render(DevicesPanel);
		await expect.element(rows().nth(2)).toHaveTextContent(/Living Room\s*Starting…/);
		const button = rows().nth(2).getByRole('button', { name: 'Play there' });
		await expect.element(button).toHaveAttribute('aria-disabled', 'true');
		// As a click would, which a browser still delivers to a button only marked disabled.
		(button.element() as HTMLElement).click();
		expect(transfer).not.toHaveBeenCalled();
	});

	it('offers nowhere to move playback while offline, nor says it is on its way', async () => {
		fake.playback.devices.push(device('4', 'Living Room', 'tv', true));
		fake.playback.moving = { deviceId: '4', commandId: 'c' };
		Object.assign(fake.realtime, { connected: false, offline: true });
		render(DevicesPanel);
		await expect.element(rows().nth(2)).toHaveTextContent(/Living Room\s*Was connected/);
		await expect.element(page.getByRole('button', { name: 'Play there' })).not.toBeInTheDocument();
	});

	it('offers nowhere to move playback before anything has played', async () => {
		fake.playback.state = {
			...playingOn('1'),
			status: 'stopped',
			activeDeviceId: null,
			trackId: null
		};
		render(DevicesPanel);
		await expect.element(rows().nth(1)).toHaveTextContent(/Desk\s*Connected/);
		await expect.element(page.getByRole('button', { name: 'Play there' })).not.toBeInTheDocument();
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
