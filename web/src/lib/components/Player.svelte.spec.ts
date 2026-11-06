import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';

const fake = vi.hoisted(() => ({
	queue: { queueId: 'q' as string | null, shuffled: false, repeat: 'off' as 'off' | 'all' | 'one' },
	player: { setShuffle: vi.fn(), setRepeat: vi.fn() },
	realtime: { connected: true, offline: false, on: () => () => {} },
	playback: {
		elsewhere: false,
		deviceName: 'Desk',
		error: null as string | null,
		libraryId: '11' as string | null
	},
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
vi.mock('$lib/player.svelte', () => ({ player: fake.player }));
vi.mock('$lib/queue/client.svelte', () => ({ queue: fake.queue }));

const { default: Player } = await import('./Player.svelte');

beforeEach(() => {
	Object.assign(fake.queue, { queueId: 'q', shuffled: false, repeat: 'off' });
	fake.player.setShuffle.mockClear();
	fake.player.setRepeat.mockClear();
	fake.controls.pickable = true;
	fake.playback.error = null;
	fake.playback.libraryId = '11';
});

describe('Player.svelte', () => {
	it('plays here where playback stopped elsewhere, and says why it could not', async () => {
		fake.playback.error = 'Could not play that here.';
		render(Player);
		await expect.element(page.getByRole('button', { name: 'Play' })).toBeEnabled();
		await expect.element(page.getByRole('alert')).toHaveTextContent('Could not play that here.');
	});

	it('hearts the song playing, in the library it plays from', async () => {
		// Wide enough for the heart, which a phone's player bar has no room for.
		await page.viewport(1280, 800);
		render(Player);
		const heart = page.getByRole('region', { name: 'Player' }).getByRole('button', {
			name: 'Like “Brain Stew”'
		});
		await expect.element(heart).toHaveAttribute('aria-pressed', 'false');
	});

	it('has no heart while the library is not known', async () => {
		// Wide enough for the heart, which a phone's player bar has no room for.
		await page.viewport(1280, 800);
		fake.playback.libraryId = null;
		render(Player);
		await expect.element(page.getByRole('button', { name: 'Shuffle' })).toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: /^Like/ })).not.toBeInTheDocument();
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

	it('turns shuffle on or off, and steps Repeat from off to all to one and back', async () => {
		await page.viewport(1024, 768);
		render(Player);
		await page.getByRole('button', { name: 'Shuffle' }).click();
		expect(fake.player.setShuffle).toHaveBeenLastCalledWith(true);
		for (const [mode, next] of [
			['off', 'all'],
			['all', 'one'],
			['one', 'off']
		] as const) {
			fake.queue.repeat = mode;
			render(Player);
			await page
				.getByRole('button', { name: `Repeat ${mode}` })
				.first()
				.click();
			expect(fake.player.setRepeat).toHaveBeenLastCalledWith(next);
		}
	});

	it('offers neither with no queue to change', async () => {
		await page.viewport(1024, 768);
		fake.queue.queueId = null;
		render(Player);
		await expect.element(page.getByRole('button', { name: 'Shuffle' })).toBeDisabled();
		await expect.element(page.getByRole('button', { name: 'Repeat off' })).toBeDisabled();
	});
});
