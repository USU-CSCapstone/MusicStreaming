import { describe, expect, it, vi } from 'vitest';
import { goto } from '$app/navigation';
import {
	type TagFilter,
	UNFILTERED,
	currentFilter,
	filterByTags,
	filterQuery,
	isFiltered,
	readFilter,
	showFiltered,
	toggleTag,
	withFilter
} from './filters';
import { readSort, withSort } from './sort-url';
import { FIELDS } from './sorting';

const at = vi.hoisted(() => ({ url: new URL('https://jewelcase.test/songs') }));
vi.mock('$app/state', () => ({ page: at }));
vi.mock('$app/navigation', () => ({ goto: vi.fn(async () => {}) }));

const params = (query: string) => new URLSearchParams(query);

describe('readFilter', () => {
	it('reads the tags in the order chosen, each once', () => {
		expect(readFilter(params('tag=30,32,30&tagMatch=all'))).toEqual({
			tags: ['30', '32'],
			match: 'all'
		});
	});

	it('matches any unless the URL says all', () => {
		expect(readFilter(params('tag=30')).match).toBe('any');
		expect(readFilter(params('tag=30&tagMatch=most')).match).toBe('any');
	});

	it('is unfiltered with no tags, or only empty ones', () => {
		expect(readFilter(params(''))).toEqual(UNFILTERED);
		expect(isFiltered(readFilter(params('tag=,')))).toBe(false);
		expect(isFiltered(readFilter(params('tag=30')))).toBe(true);
	});
});

describe('toggleTag', () => {
	it('adds a tag at the end, and takes a chosen one out', () => {
		const rock = toggleTag(UNFILTERED, '30');
		const both = toggleTag(rock, '32');
		expect(both.tags).toEqual(['30', '32']);
		expect(toggleTag(both, '30')).toEqual({ tags: ['32'], match: 'any' });
	});

	it('keeps the match', () => {
		expect(toggleTag({ tags: [], match: 'all' }, '30').match).toBe('all');
	});
});

describe('filterQuery', () => {
	it("is the API's parameters, leaving out its default match", () => {
		expect(filterQuery({ tags: ['30', '32'], match: 'any' })).toEqual({
			tag: '30,32'
		});
		expect(filterQuery({ tags: ['30'], match: 'all' })).toEqual({
			tag: '30',
			tagMatch: 'all'
		});
	});

	it('is nothing for no filter, whatever its match', () => {
		expect(filterQuery({ tags: [], match: 'all' })).toEqual({});
	});
});

describe('withFilter', () => {
	const url = new URL('https://jewelcase.test/songs?sort=artist&order=desc#top');

	it('sets the filter and keeps the rest of the URL, its sort included', () => {
		const filtered = withFilter(url, { tags: ['30', '32'], match: 'all' });
		expect(filtered.href).toBe(
			'https://jewelcase.test/songs?sort=artist&order=desc&tag=30%2C32&tagMatch=all#top'
		);
		expect(readSort(filtered.searchParams, FIELDS)).toEqual({
			field: 'artist',
			order: 'desc'
		});
		expect(url.href).toBe('https://jewelcase.test/songs?sort=artist&order=desc#top');
	});

	it('round-trips through readFilter, and a sort keeps it', () => {
		const filter: TagFilter = { tags: ['32', '30'], match: 'all' };
		const sorted = withSort(withFilter(url, filter), {
			field: 'title',
			order: 'asc'
		});
		expect(readFilter(sorted.searchParams)).toEqual(filter);
	});

	it('takes the filter out when unfiltered', () => {
		const filtered = withFilter(url, { tags: ['30'], match: 'all' });
		expect(withFilter(filtered, UNFILTERED).href).toBe(url.href);
	});
});

describe("this page's filter", () => {
	it('is what the URL holds', () => {
		at.url = new URL('https://jewelcase.test/albums?tag=31');
		expect(currentFilter()).toEqual({ tags: ['31'], match: 'any' });
	});

	it('changes in place: the same history entry, focus, and scroll', async () => {
		at.url = new URL('https://jewelcase.test/songs?sort=title&order=asc');
		await showFiltered({ tags: ['30'], match: 'any' });
		expect(goto).toHaveBeenLastCalledWith(
			new URL('https://jewelcase.test/songs?sort=title&order=asc&tag=30'),
			{ replaceState: true, keepFocus: true, noScroll: true }
		);
	});
});

describe('filterByTags', () => {
	type Item = { name: string; tags: Record<string, { id: string; name: string }[]> };
	const item = (name: string, genres: string[], moods: string[] = []): Item => ({
		name,
		tags: {
			...(genres.length && { genre: genres.map((id) => ({ id, name: id })) }),
			...(moods.length && { mood: moods.map((id) => ({ id, name: id })) })
		}
	});
	// Rock is 30, Jazz 31, Calm 32 and Pop 34, which no item carries.
	const items = [
		item('daybreak', ['30'], ['32']),
		item('static', ['30']),
		item('drift', ['31'], ['32']),
		item('pulse', ['30', '31'])
	];
	const kept = (tags: string[], match: 'any' | 'all' = 'any', typeOf?: (tag: string) => string) =>
		filterByTags(items, { tags, match }, (i) => i.tags, typeOf).map((i) => i.name);

	it('keeps everything with no filter', () => {
		expect(kept([])).toEqual(['daybreak', 'static', 'drift', 'pulse']);
	});

	it('matches any of one type, or all of it', () => {
		expect(kept(['30', '31'])).toEqual(['daybreak', 'static', 'drift', 'pulse']);
		expect(kept(['30', '31'], 'all')).toEqual(['pulse']);
	});

	it('matches every type named', () => {
		expect(kept(['30', '32'])).toEqual(['daybreak']);
		expect(kept(['31', '30', '32'])).toEqual(['daybreak', 'drift']);
	});

	it('groups a tag no item carries by the type it is known to have', () => {
		const genre = () => 'genre';
		expect(kept(['30', '34'], 'any', genre)).toEqual(['daybreak', 'static', 'pulse']);
		expect(kept(['30', '34'], 'all', genre)).toEqual([]);
		// Not known, or not the library's, it matches nothing.
		expect(kept(['30', '34'])).toEqual([]);
	});
});
