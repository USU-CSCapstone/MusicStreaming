import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const fake = vi.hoisted(() => ({
	realtime: { connected: true, offline: false },
	playback: { elsewhere: false, deviceName: 'Desk', error: null as string | null },
	controls: {
		remote: true,
		stopped: true,
		pickable: true,
		playing: false,
		track: {
			id: '41',
			title: 'Brain Stew',
			artists: [{ id: '21', name: 'Green Day' }],
			album: { id: '31', title: 'Insomniac', artists: [], image: null }
		} as unknown,
		position: 10,
		duration: 193,
		waveform: null,
		volume: 1,
		toggle: () => {}
	}
}));

vi.mock('$app/state', () => ({ page: { data: { library: { id: '11' } } } }));
vi.mock('$lib/realtime.svelte', () => ({ realtime: fake.realtime }));
vi.mock('$lib/playback.svelte', () => ({ playback: fake.playback }));
vi.mock('$lib/controls.svelte', () => ({ controls: fake.controls }));

const { default: Player } = await import('./Player.svelte');

beforeEach(() => {
	fake.controls.pickable = true;
	fake.playback.error = null;
});

describe('Player.svelte', () => {
	it('plays here where playback stopped elsewhere, and says why it could not', async () => {
		fake.playback.error = 'Could not play that here.';
		render(Player);
		await expect.element(page.getByRole('button', { name: 'Play' })).toBeEnabled();
		await expect.element(page.getByRole('alert')).toHaveTextContent('Could not play that here.');
	});

	it('cannot play where playback stopped in a library this device does not play', async () => {
		fake.controls.pickable = false;
		render(Player);
		await expect.element(page.getByRole('button', { name: 'Play' })).toBeDisabled();
	});

	it('says where playback is as the device list does, through a blip in the connection', async () => {
		Object.assign(fake.controls, { stopped: false });
		Object.assign(fake.realtime, { connected: false, offline: false });
		render(Player);
		await expect.element(page.getByText('On Desk', { exact: true })).toBeVisible();
		fake.realtime.offline = true;
		render(Player);
		await expect.element(page.getByText('Last on Desk')).toBeVisible();
		Object.assign(fake.controls, { stopped: true });
		Object.assign(fake.realtime, { connected: true, offline: false });
	});

	it('says why a command failed with nothing playing here', async () => {
		fake.playback.error = 'That device is not connected.';
		const { remote, track } = fake.controls;
		Object.assign(fake.controls, { remote: false, track: null });
		render(Player);
		await expect
			.element(page.getByRole('alert'))
			.toHaveTextContent('That device is not connected.');
		await expect.element(page.getByText('Nothing playing')).not.toBeInTheDocument();
		Object.assign(fake.controls, { remote, track });
	});
});
