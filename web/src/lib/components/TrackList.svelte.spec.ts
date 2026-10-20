import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { TrackSummary } from '$lib/api/types';
import type { TrackSort } from '$lib/sorting';
import TrackList from './TrackList.svelte';

const track = (id: string, title: string): TrackSummary => ({
	id,
	title,
	artists: [{ id: '21', name: 'Aurora Lane' }],
	album: { id: '31', title: 'Signal', artists: [], image: null },
	discNumber: 1,
	trackNumber: Number(id),
	durationUs: 2_000_000,
	releaseDate: '2023',
	explicit: false,
	audio: { codec: 'flac', lossless: true },
	availability: 'available',
	sortKeys: { title: title.toLowerCase(), artist: 'aurora lane', album: 'signal' }
});
const tracks = [track('1', 'Signal Part 1'), track('2', 'Signal Part 2')];

const heading = (name: string | RegExp) => page.getByRole('button', { name });

describe('TrackList.svelte header', () => {
	beforeEach(async () => {
		await page.viewport(1280, 800);
	});

	it('heads every column, and a click steps its sort', async () => {
		const onsort = vi.fn<(sort: TrackSort) => void>();
		const list = render(TrackList, { tracks, onsort });
		for (const name of ['Title', 'Artist', 'Album', 'Year']) {
			await expect.element(heading(name)).toBeVisible();
		}
		await expect.element(heading('Duration')).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Date added' })).not.toBeInTheDocument();

		await heading('Artist').click();
		expect(onsort).toHaveBeenLastCalledWith({ field: 'artist', order: 'asc' });

		await list.rerender({ sort: { field: 'artist', order: 'asc' } });
		await heading('Artist, sorted ascending').click();
		expect(onsort).toHaveBeenLastCalledWith({ field: 'artist', order: 'desc' });

		await list.rerender({ sort: { field: 'artist', order: 'desc' } });
		await heading('Artist, sorted descending').click();
		expect(onsort).toHaveBeenLastCalledWith(null);
		await expect.element(page.getByText('Unsorted')).toBeInTheDocument();

		// Another column starts over.
		await heading('Year').click();
		expect(onsort).toHaveBeenLastCalledWith({ field: 'year', order: 'asc' });
	});

	it('heads date added on a playlist', async () => {
		const added = ['2026-01-01T00:00:00.000Z', '2026-02-01T00:00:00.000Z'];
		const onsort = vi.fn();
		render(TrackList, { tracks, added, onsort });
		await heading('Date added').click();
		expect(onsort).toHaveBeenLastCalledWith({ field: 'added', order: 'asc' });
	});

	it("heads no album or year on an album's own page", async () => {
		render(TrackList, { tracks, variant: 'album', onsort: () => {} });
		await expect.element(heading('Title')).toBeVisible();
		await expect.element(heading('Artist')).toBeVisible();
		await expect.element(page.getByRole('button', { name: 'Album' })).not.toBeInTheDocument();
		await expect.element(page.getByRole('button', { name: 'Year' })).not.toBeInTheDocument();
	});

	it('only labels the columns of a list that does not sort', async () => {
		render(TrackList, { tracks });
		await expect.element(page.getByRole('group', { name: 'Sort songs' })).toBeVisible();
		expect(page.getByRole('group').getByRole('button').elements()).toHaveLength(0);
	});

	it('is not shown where the columns do not fit', async () => {
		await page.viewport(600, 800);
		render(TrackList, { tracks, onsort: () => {} });
		await expect.element(page.getByText('Signal Part 1')).toBeVisible();
		// Hidden, so out of reach by role.
		await expect.element(page.getByText('Title', { exact: true })).not.toBeVisible();
	});
});
