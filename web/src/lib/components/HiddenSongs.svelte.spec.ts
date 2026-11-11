import { page } from 'vitest/browser';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { TrackSummary } from '$lib/api/types';

vi.mock('$app/state', () => ({ page: { data: { library: { id: '1' } } } }));
const sent = vi.hoisted(() => [] as string[]);
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	hideTrack: async (_f: unknown, _l: string, id: string) => void sent.push(`hide ${id}`),
	unhideTrack: async (_f: unknown, _l: string, id: string) => void sent.push(`unhide ${id}`)
}));

const { marks } = await import('$lib/marks.svelte');
const { sender } = await import('$lib/sync/sender');
const { memoryEnv } = await import('$lib/sync/store');
const { toaster } = await import('$lib/toast.svelte');
const { default: HiddenSongs } = await import('./HiddenSongs.svelte');

const track = (id: string, title: string) =>
	({
		id,
		title,
		artists: [{ id: '21', name: 'Aurora Lane' }],
		album: { id: '31', title: 'Signal', artists: [], image: null },
		liked: false
	}) as unknown as TrackSummary;
const first = {
	items: [track('2', 'Skit'), track('1', 'Interlude')],
	nextCursor: null,
	total: 2
};

beforeEach(async () => {
	marks.clear();
	sent.length = 0;
	sender.start('1', { env: memoryEnv() });
});

afterEach(() => {
	toaster.dismiss();
	sender.stop();
});

describe('HiddenSongs.svelte', () => {
	it('lists the hidden songs under a count, and Unhide brings one back, with Undo', async () => {
		render(HiddenSongs, { library: '1', first, since: 0 });
		await expect.element(page.getByRole('heading', { name: 'Hidden Songs' })).toBeVisible();
		// A list to unhide from, not to play.
		await expect.element(page.getByRole('button', { name: 'Play' })).not.toBeInTheDocument();
		await expect.element(page.getByText('2 songs')).toBeVisible();
		await expect.element(page.getByText('Skit')).toBeVisible();

		await page.getByRole('button', { name: 'Unhide “Skit”' }).click();
		await expect.element(page.getByText('Skit')).not.toBeInTheDocument();
		await expect.element(page.getByText('1 song', { exact: true })).toBeVisible();
		expect(toaster.current?.message).toBe('“Skit” is back in your library');

		toaster.current?.action?.run();
		await expect.element(page.getByText('Skit')).toBeVisible();
		expect(sent).toEqual(['unhide 2', 'hide 2']);
	});

	it('leaves out a song liked elsewhere since, which unhid it', async () => {
		render(HiddenSongs, { library: '1', first, since: 0 });
		marks.receive({ type: 'trackLiked', libraryId: '1', trackId: '1', liked: true });
		await expect.element(page.getByText('Interlude')).not.toBeInTheDocument();
	});

	it('says when there are none, and how to hide one', async () => {
		render(HiddenSongs, {
			library: '1',
			first: { items: [], nextCursor: null, total: 0 },
			since: 0
		});
		await expect
			.element(page.getByText(/^You haven't hidden any songs in this library\./))
			.toBeVisible();
	});
});
