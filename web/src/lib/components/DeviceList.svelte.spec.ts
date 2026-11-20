import { page, userEvent } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Device } from '$lib/api/types';
import { connectivity } from '$lib/connectivity.svelte';

const fake = vi.hoisted(() => ({
	realtime: { deviceId: '2', connected: true },
	playback: {
		thisDevice: '2',
		devices: [] as unknown[],
		state: null as unknown,
		moving: null as unknown,
		transfer: () => {}
	},
	controls: {
		playing: false,
		remote: false,
		track: null,
		item: null,
		pickable: false,
		playHere: () => {}
	},
	signedOut: [] as string[],
	failure: null as Error | null,
	hold: null as Promise<void> | null
}));

vi.mock('$lib/realtime.svelte', () => ({ realtime: fake.realtime }));
vi.mock('$lib/playback.svelte', () => ({ playback: fake.playback }));
vi.mock('$lib/controls.svelte', () => ({ controls: fake.controls }));
vi.mock('$lib/api/client', async () => ({
	ApiError: (await import('$lib/api/root')).ApiError,
	signOutMyDevice: async (_fetch: unknown, id: string) => {
		fake.signedOut.push(id);
		await fake.hold;
		if (fake.failure) throw fake.failure;
	}
}));

const playback = $state(fake.playback);
Object.assign(fake, { playback });

const { ApiError } = await import('$lib/api/root');
const { default: DeviceList } = await import('./DeviceList.svelte');

const device = (id: string, name: string, connected = true) =>
	({ id, name, type: 'desktop', connected, firstSeenAt: '', lastSeenAt: '' }) as Device;

beforeEach(() => {
	connectivity.reach();
	fake.signedOut.length = 0;
	fake.failure = null;
	fake.hold = null;
	document.body.style.cssText = '';
	fake.controls.remote = false;
	fake.controls.pickable = false;
	fake.playback.devices = [device('1', 'Desk'), device('2', 'Phone'), device('3', 'Old', false)];
});

const rows = () => page.getByRole('list', { name: 'Devices' }).getByRole('listitem');

describe('DeviceList sign-out controls', () => {
	it('offers Sign out on every device but this one, which points at Log out', async () => {
		render(DeviceList, { signOut: true });
		await expect.element(rows().nth(0)).toHaveTextContent(/Phone/);
		await expect.element(rows().nth(0)).toHaveTextContent(/Log out/);
		await expect.element(rows().nth(0).getByRole('button')).not.toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Sign out Desk' })).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Sign out Old' })).toBeVisible();
		expect(page.getByRole('button', { name: 'Sign out Phone' }).elements()).toHaveLength(0);
	});

	it('shows no sign-out control without the prop', async () => {
		render(DeviceList);
		await expect.element(rows().nth(0)).toBeVisible();
		expect(page.getByRole('button', { name: /Sign out/ }).elements()).toHaveLength(0);
		expect(page.getByText(/Log out/).elements()).toHaveLength(0);
	});

	it("drops a signed-out device only when the server's new list arrives", async () => {
		render(DeviceList, { signOut: true });
		await page.getByRole('button', { name: 'Sign out Desk' }).click();
		await vi.waitFor(() => expect(fake.signedOut).toEqual(['1']));
		// The request is answered, but the list is the server's to change.
		await expect.element(page.getByText('Desk', { exact: true })).toBeVisible();
		fake.playback.devices = [device('2', 'Phone'), device('3', 'Old', false)];
		await expect.element(page.getByText('Desk', { exact: true })).not.toBeInTheDocument();
	});

	it("shows the server's reason when a sign-out is refused, and keeps the device", async () => {
		fake.failure = new ApiError(404, 'not_found', 'That device is already signed out.');
		render(DeviceList, { signOut: true });
		await page.getByRole('button', { name: 'Sign out Desk' }).click();
		await expect
			.element(page.getByRole('alert'))
			.toHaveTextContent('That device is already signed out.');
		await expect.element(page.getByText('Desk', { exact: true })).toBeVisible();
	});

	it('keeps keyboard focus on Sign out while the request runs, and asks once', async () => {
		let release = () => {};
		fake.hold = new Promise<void>((done) => (release = done));
		render(DeviceList, { signOut: true });
		const button = page.getByRole('button', { name: 'Sign out Desk' });
		(button.element() as HTMLElement).focus();
		await userEvent.keyboard('{Enter}');
		await vi.waitFor(() => expect(fake.signedOut).toEqual(['1']));
		expect(document.activeElement).toBe(button.element());
		await userEvent.keyboard('{Enter}');
		expect(fake.signedOut).toEqual(['1']);
		release();
	});

	it('keeps this device’s name readable at phone width beside Play here and the Log out note', async () => {
		// The Account page's width on a phone: 375 px less its 16 px gutters.
		document.body.style.cssText = 'width: 343px; margin: 0';
		fake.controls.remote = true;
		fake.controls.pickable = true;
		fake.playback.devices = [
			device('2', 'Sam’s iPhone 15 Pro'),
			device('1', 'Living room desktop')
		];
		render(DeviceList, { signOut: true });
		const name = page.getByText('Sam’s iPhone 15 Pro', { exact: true }).element();
		await expect.element(page.getByRole('button', { name: 'Play here' })).toBeVisible();
		expect(name.getBoundingClientRect().width).toBeGreaterThan(150);
	});
});
