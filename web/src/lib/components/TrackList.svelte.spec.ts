import { page, userEvent } from 'vitest/browser';
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
	tags: {},
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

	it('leaves the menu to narrow lists', async () => {
		render(TrackList, { tracks, onsort: () => {} });
		await expect.element(heading('Title')).toBeVisible();
		await expect.element(page.getByText('Sort', { exact: true })).not.toBeVisible();
	});
});

describe('TrackList.svelte sort menu', () => {
	const sortButton = (name: string) => page.getByRole('button', { name, exact: true });
	const item = (name: string) => page.getByRole('menuitemradio', { name: new RegExp(`^${name}`) });

	beforeEach(async () => {
		await page.viewport(400, 800);
	});

	it('steps the sort as the headings do', async () => {
		const onsort = vi.fn<(sort: TrackSort) => void>();
		const list = render(TrackList, { tracks, onsort });

		await sortButton('Sort').click();
		await expect.element(item('Duration')).toBeVisible();
		await item('Artist').click();
		expect(onsort).toHaveBeenLastCalledWith({ field: 'artist', order: 'asc' });
		await expect.element(page.getByRole('menu')).not.toBeInTheDocument();

		await list.rerender({ sort: { field: 'artist', order: 'asc' } });
		await sortButton('Sort, by artist ascending').click();
		await expect.element(item('Artist')).toHaveAttribute('aria-checked', 'true');
		await expect.element(item('Title')).toHaveAttribute('aria-checked', 'false');
		await item('Artist').click();
		expect(onsort).toHaveBeenLastCalledWith({ field: 'artist', order: 'desc' });

		await list.rerender({ sort: { field: 'artist', order: 'desc' } });
		await sortButton('Sort, by artist descending').click();
		await item('Artist').click();
		expect(onsort).toHaveBeenLastCalledWith(null);
	});

	it('offers date added on a playlist, and only its own columns on an album', async () => {
		const added = ['2026-01-01T00:00:00.000Z', '2026-02-01T00:00:00.000Z'];
		const playlist = render(TrackList, { tracks, added, onsort: () => {} });
		await sortButton('Sort').click();
		await expect.element(item('Date added')).toBeVisible();
		await userEvent.keyboard('{Escape}');
		playlist.unmount();

		render(TrackList, { tracks, variant: 'album', onsort: () => {} });
		await sortButton('Sort').click();
		expect(
			page
				.getByRole('menuitemradio')
				.elements()
				.map((e) => e.textContent?.trim())
		).toEqual(['Title', 'Artist', 'Duration']);
	});

	it('works from the keyboard, and gives focus back on close', async () => {
		const onsort = vi.fn();
		render(TrackList, { tracks, sort: { field: 'year', order: 'asc' }, onsort });
		await sortButton('Sort, by year ascending').click();
		// Focus starts on the sort in use.
		await expect.element(item('Year')).toHaveFocus();
		await userEvent.keyboard('{ArrowDown}');
		await expect.element(item('Duration')).toHaveFocus();
		await userEvent.keyboard('{Escape}');
		await expect.element(page.getByRole('menu')).not.toBeInTheDocument();
		await expect.element(sortButton('Sort, by year ascending')).toHaveFocus();
		expect(onsort).not.toHaveBeenCalled();
	});

	it('closes on Tab, and the same press moves on past it', async () => {
		render(TrackList, { tracks, onsort: () => {} });
		await sortButton('Sort').click();
		await expect.element(item('Title')).toHaveFocus();
		await userEvent.keyboard('{Tab}');
		await expect.element(page.getByRole('menu')).not.toBeInTheDocument();
		await expect.element(sortButton('Signal Part 1')).toHaveFocus();

		await sortButton('Sort').click();
		await userEvent.keyboard('{Shift>}{Tab}{/Shift}');
		await expect.element(page.getByRole('menu')).not.toBeInTheDocument();
		await expect.element(sortButton('Sort')).not.toHaveFocus();
	});

	it('stays in view as the list scrolls', async () => {
		const many = Array.from({ length: 60 }, (_, k) => track(String(k + 1), `Song ${k + 1}`));
		render(TrackList, { tracks: many, onsort: () => {} });
		await expect.element(sortButton('Sort')).toBeVisible();
		scrollTo(0, 2000);
		await expect.poll(() => scrollY).toBeGreaterThan(1000);
		const { top } = sortButton('Sort').element().getBoundingClientRect();
		expect(top).toBeGreaterThanOrEqual(0);
		expect(top).toBeLessThan(40);
	});

	it('is not offered on a list that does not sort', async () => {
		render(TrackList, { tracks });
		await expect.element(page.getByText('Signal Part 1')).toBeVisible();
		await expect.element(sortButton('Sort')).not.toBeInTheDocument();
	});
});

describe('TrackList.svelte reordering', () => {
	beforeEach(async () => {
		await page.viewport(1280, 800);
	});

	it('moves rows in the list’s own order', async () => {
		const onmove = vi.fn();
		render(TrackList, { tracks, editable: true, onmove, onsort: () => {} });
		await page.getByText('Signal Part 1').click();
		await userEvent.keyboard('{Alt>}{ArrowDown}{/Alt}');
		expect(onmove).toHaveBeenCalledWith([{ indexes: [0], to: 1 }]);
	});

	it('moves nothing while sorted, and says why', async () => {
		const onmove = vi.fn();
		render(TrackList, {
			tracks,
			editable: true,
			onmove,
			sort: { field: 'title', order: 'desc' },
			onsort: () => {}
		});
		await page.getByText('Signal Part 1').click();
		await userEvent.keyboard('{Alt>}{ArrowDown}{/Alt}');
		expect(onmove).not.toHaveBeenCalled();
		await expect
			.element(page.getByText('The list is sorted, so songs can’t be moved'))
			.toBeInTheDocument();
	});
});

describe('TrackList.svelte on an album', () => {
	const discs = [
		{ ...track('1', 'Signal Part 1'), discNumber: 1 },
		{ ...track('2', 'Signal Part 2'), discNumber: 1 },
		{ ...track('3', 'Signal Remix'), discNumber: 2, trackNumber: 1 }
	];

	it('heads each disc in disc order', async () => {
		render(TrackList, { tracks: discs, variant: 'album', onsort: () => {} });
		await expect.element(page.getByText('Disc 1')).toBeVisible();
		await expect.element(page.getByText('Disc 2')).toBeVisible();
	});

	it('runs together when sorted', async () => {
		render(TrackList, {
			tracks: discs.toReversed(),
			variant: 'album',
			sort: { field: 'title', order: 'desc' },
			onsort: () => {}
		});
		await expect.element(page.getByText('Signal Remix')).toBeVisible();
		await expect.element(page.getByText('Disc 1')).not.toBeInTheDocument();
		await expect.element(page.getByText('Disc 2')).not.toBeInTheDocument();
	});
});
