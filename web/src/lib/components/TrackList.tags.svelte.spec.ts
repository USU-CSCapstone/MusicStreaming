import { page } from 'vitest/browser';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Tag, TagType, TrackSummary } from '$lib/api/types';
import { type TagFilter, UNFILTERED } from '$lib/filters';

const at = vi.hoisted(() => ({
	data: { library: { id: '1' }, tagTypes: [] as TagType[] },
	url: new URL('https://jewelcase.test/songs')
}));
vi.mock('$app/state', () => ({ page: at }));

const tag = (id: string, type: string, name: string): Tag => ({
	id,
	type,
	name,
	trackCount: 1,
	albumCount: 1,
	artistCount: 1
});
const TAGS = [tag('30', 'genre', 'Rock'), tag('31', 'genre', 'Jazz'), tag('32', 'mood', 'Calm')];
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	listTags: async (_f: unknown, _l: string, query: { type: string }) => {
		const items = TAGS.filter((t) => t.type === query.type);
		return { items, nextCursor: null, total: items.length };
	},
	getTag: async (_f: unknown, _l: string, id: string) => TAGS.find((t) => t.id === id)!
}));

const { prefs } = await import('$lib/prefs.svelte');
const { default: TrackList } = await import('./TrackList.svelte');

const track = (id: string, title: string, tags: Tag[]): TrackSummary => ({
	id,
	title,
	artists: [{ id: '21', name: 'Aurora Lane' }],
	album: { id: '31', title: 'Signal', artists: [], image: null },
	discNumber: 1,
	trackNumber: Number(id),
	durationUs: 2_000_000,
	releaseDate: '2023',
	tags: Object.groupBy(tags, (t) => t.type) as TrackSummary['tags'],
	explicit: false,
	audio: { codec: 'flac', lossless: true },
	availability: 'available',
	sortKeys: { title: title.toLowerCase(), artist: 'aurora lane', album: 'signal' }
});
const tracks = [
	track('1', 'Daybreak', [TAGS[0], TAGS[2]]),
	track('2', 'Drift', [TAGS[1], TAGS[0]])
];

describe('TrackList.svelte tag columns', () => {
	beforeEach(async () => {
		await page.viewport(1280, 800);
		at.data.tagTypes = [
			{ key: 'genre', tagCount: 2 },
			{ key: 'mood', tagCount: 1 }
		];
		prefs.showTagColumn('genre', true);
		prefs.showTagColumn('mood', true);
	});

	it("shows each type's tags as links, under a heading that only names it", async () => {
		render(TrackList, { tracks });
		await expect.element(page.getByText('Genre', { exact: true })).toBeVisible();
		await expect.element(page.getByText('Mood', { exact: true })).toBeVisible();
		const rock = page.getByRole('link', { name: 'Rock' });
		await expect
			.element(rock.first())
			.toHaveAttribute('href', expect.stringMatching(/\/tags\/30$/));
		await expect.element(page.getByRole('link', { name: 'Calm' })).toBeVisible();
	});

	it('filters from the heading where the list does', async () => {
		const changes: TagFilter[] = [];
		render(TrackList, { tracks, filter: UNFILTERED, onfilter: (f: TagFilter) => changes.push(f) });
		await page.getByRole('button', { name: 'Filter by genre' }).click();
		await page.getByRole('checkbox', { name: /Jazz/ }).click();
		expect(changes).toEqual([{ tags: ['31'], match: 'any' }]);
	});

	it('leaves out a hidden type, unless the filter names it', async () => {
		prefs.showTagColumn('mood', false);
		render(TrackList, { tracks });
		await expect.element(page.getByText('Genre', { exact: true })).toBeVisible();
		await expect.element(page.getByText('Mood', { exact: true })).not.toBeInTheDocument();

		render(TrackList, { tracks, filter: { tags: ['32'], match: 'any' }, onfilter: () => {} });
		await expect
			.element(page.getByRole('button', { name: 'Filter by mood, 1 chosen' }))
			.toBeVisible();
	});

	it('offers to clear a filter that leaves nothing', async () => {
		const changes: TagFilter[] = [];
		render(TrackList, {
			tracks: [],
			empty: 'No songs carry these tags.',
			filter: { tags: ['31', '32'], match: 'all' },
			onfilter: (f: TagFilter) => changes.push(f)
		});
		await page.getByRole('button', { name: 'Clear filters' }).click();
		expect(changes).toEqual([UNFILTERED]);
	});
});
