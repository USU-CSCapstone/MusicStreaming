import { page } from 'vitest/browser';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { TrackSummary } from '$lib/api/types';

const at = vi.hoisted(() => ({
	data: { library: { id: '1' }, tagTypes: [] },
	url: new URL('https://jewelcase.test/songs')
}));
vi.mock('$app/state', () => ({ page: at }));

// What each like and hide sent, by track.
const sent = vi.hoisted(() => [] as string[]);
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	listPlaylists: async () => ({ items: [], nextCursor: null, total: 0 }),
	likeTrack: async (_f: unknown, _l: string, id: string) => void sent.push(`like ${id}`),
	unlikeTrack: async (_f: unknown, _l: string, id: string) => void sent.push(`unlike ${id}`),
	hideTrack: async (_f: unknown, _l: string, id: string) => void sent.push(`hide ${id}`),
	unhideTrack: async (_f: unknown, _l: string, id: string) => void sent.push(`unhide ${id}`)
}));

const { marks } = await import('$lib/marks.svelte');
const { player } = await import('$lib/player.svelte');
const { toaster } = await import('$lib/toast.svelte');
const { default: TrackList } = await import('./TrackList.svelte');

const track = (id: string, title: string, liked = false): TrackSummary => ({
	id,
	title,
	artists: [{ id: '21', name: 'Aurora Lane' }],
	album: { id: '31', title: 'Signal', artists: [], image: null },
	discNumber: 1,
	trackNumber: Number(id),
	durationUs: 2_000_000,
	releaseDate: '2023',
	tags: {},
	explicit: false,
	audio: { codec: 'flac', lossless: true },
	availability: 'available',
	sortKeys: { title: title.toLowerCase(), artist: 'aurora lane', album: 'signal' },
	liked
});
const tracks = [track('1', 'Signal Part 1', true), track('2', 'Signal Part 2')];
const heart = (title: string) => page.getByRole('button', { name: `Like “${title}”` });

beforeEach(async () => {
	await page.viewport(1280, 800);
	marks.clear();
	sent.length = 0;
});

afterEach(() => {
	toaster.dismiss();
	vi.restoreAllMocks();
});

describe('TrackList.svelte likes', () => {
	it('gives each row a heart, pressed when liked, that likes and unlikes without playing', async () => {
		const play = vi.spyOn(player, 'play').mockImplementation(() => {});
		render(TrackList, { tracks });
		await expect.element(heart('Signal Part 1')).toHaveAttribute('aria-pressed', 'true');
		await expect.element(heart('Signal Part 2')).toHaveAttribute('aria-pressed', 'false');

		await heart('Signal Part 2').click();
		await expect.element(heart('Signal Part 2')).toHaveAttribute('aria-pressed', 'true');
		await heart('Signal Part 1').click();
		await expect.element(heart('Signal Part 1')).toHaveAttribute('aria-pressed', 'false');
		expect(sent).toEqual(['like 2', 'unlike 1']);
		expect(play).not.toHaveBeenCalled();
	});

	it('likes and hides a song from its menu, a hide with Undo', async () => {
		render(TrackList, { tracks });
		await page.getByRole('button', { name: 'Actions for Signal Part 2' }).click();
		await page.getByRole('menuitem', { name: 'Add to Liked Songs' }).click();
		await expect.element(heart('Signal Part 2')).toHaveAttribute('aria-pressed', 'true');

		await page.getByRole('button', { name: 'Actions for Signal Part 2' }).click();
		await expect
			.element(page.getByRole('menuitem', { name: 'Remove from Liked Songs' }))
			.toBeVisible();
		await page.getByRole('menuitem', { name: 'Hide song' }).click();
		expect(marks.hidden('1', '2')).toBe(true);
		expect(marks.liked('1', tracks[1])).toBe(false);
		expect(toaster.current?.message).toBe('Hid “Signal Part 2”');
		toaster.current?.action?.run();
		await vi.waitFor(() => expect(marks.hidden('1', '2')).toBe(false));
		expect(sent).toEqual(['like 2', 'hide 2', 'unhide 2']);
	});

	it('offers no like or hide for a selection', async () => {
		render(TrackList, { tracks, editable: true });
		await page.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
		await page
			.getByRole('button', { name: 'Signal Part 2', exact: true })
			.click({ modifiers: ['Shift'] });
		await page.getByRole('button', { name: 'Add to queue or playlist' }).click();
		await expect.element(page.getByRole('menuitem', { name: 'Queue next' })).toBeVisible();
		await expect.element(page.getByRole('menuitem', { name: 'Hide song' })).not.toBeInTheDocument();
	});
});
