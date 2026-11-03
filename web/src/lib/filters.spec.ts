import { describe, expect, it, vi } from 'vitest';
import { goto } from '$app/navigation';
import {
	type TagFilter,
	UNFILTERED,
	currentFilter,
	filterByTags,
	filterQuery,
	isFiltered,
	matchTags,
	readFilter,
	showFiltered,
	shownFilter,
	tagTypesKnown,
	toggleTag,
	typeMatch,
	withFilter,
	withoutTags
} from './filters';
import { readSort, withSort } from './sort-url';
import { FIELDS } from './sorting';

const at = vi.hoisted(() => ({ url: new URL('https://jewelcase.test/songs') }));
vi.mock('$app/state', () => ({ page: at }));
vi.mock('$app/navigation', () => ({ goto: vi.fn(async () => {}) }));

const params = (query: string) => new URLSearchParams(query);

describe('readFilter', () => {
	it("reads each list's tags in the order chosen, each once", () => {
		expect(readFilter(params('tagAll=30,32,30&tagAny=31,34,31'))).toEqual({
			all: ['30', '32'],
			any: ['31', '34']
		});
	});

	it('is unfiltered with no tags, or only empty ones', () => {
		expect(readFilter(params(''))).toEqual(UNFILTERED);
		expect(isFiltered(readFilter(params('tagAll=,&tagAny=')))).toBe(false);
		expect(isFiltered(readFilter(params('tagAll=30')))).toBe(true);
		expect(isFiltered(readFilter(params('tagAny=30')))).toBe(true);
	});
});

// Rock is 30 and Jazz 31, genres; Calm is 32, a mood.
describe("a type's tags", () => {
	it('match all where all are in `all`, any where all are in `any`, and neither for a mix', () => {
		const filter: TagFilter = { all: ['30', '31'], any: ['32'] };
		expect(typeMatch(filter, ['30', '31'])).toBe('all');
		expect(typeMatch(filter, ['32'])).toBe('any');
		expect(typeMatch({ all: ['30'], any: ['31'] }, ['30', '31'])).toBeUndefined();
		expect(typeMatch(filter, [])).toBeUndefined();
	});

	it('take a new tag into the list the type is in, any for a type with none', () => {
		const rock = toggleTag(UNFILTERED, '30', []);
		expect(rock).toEqual({ all: [], any: ['30'] });
		expect(toggleTag(rock, '31', ['30'])).toEqual({ all: [], any: ['30', '31'] });
		const all: TagFilter = { all: ['30'], any: ['32'] };
		expect(toggleTag(all, '31', ['30'])).toEqual({ all: ['30', '31'], any: ['32'] });
	});

	it('let a chosen tag go from whichever list holds it', () => {
		expect(toggleTag({ all: ['30'], any: ['31', '32'] }, '31', ['30', '31'])).toEqual({
			all: ['30'],
			any: ['32']
		});
	});

	it('move together to all or any, keeping their order and the other types', () => {
		const filter: TagFilter = { all: [], any: ['30', '32', '31'] };
		const all = matchTags(filter, ['30', '31'], 'all');
		expect(all).toEqual({ all: ['30', '31'], any: ['32'] });
		expect(matchTags(all, ['30', '31'], 'any')).toEqual({ all: [], any: ['32', '30', '31'] });
	});

	it('clear without touching the other types', () => {
		expect(withoutTags({ all: ['30'], any: ['31', '32'] }, ['30', '31'])).toEqual({
			all: [],
			any: ['32']
		});
	});
});

describe('filterQuery', () => {
	it("is the API's parameters, leaving out a list with no tags", () => {
		expect(filterQuery({ all: [], any: ['30', '32'] })).toEqual({ tagAny: '30,32' });
		expect(filterQuery({ all: ['30', '31'], any: ['32'] })).toEqual({
			tagAll: '30,31',
			tagAny: '32'
		});
	});

	it('is nothing for no filter', () => {
		expect(filterQuery(UNFILTERED)).toEqual({});
	});
});

describe('withFilter', () => {
	const url = new URL('https://jewelcase.test/songs?sort=artist&order=desc#top');

	it('sets the filter and keeps the rest of the URL, its sort included', () => {
		const filtered = withFilter(url, { all: ['30'], any: ['31', '32'] });
		expect(filtered.href).toBe(
			'https://jewelcase.test/songs?sort=artist&order=desc&tagAll=30&tagAny=31%2C32#top'
		);
		expect(readSort(filtered.searchParams, FIELDS)).toEqual({
			field: 'artist',
			order: 'desc'
		});
		expect(url.href).toBe('https://jewelcase.test/songs?sort=artist&order=desc#top');
	});

	it('round-trips through readFilter, and a sort keeps it', () => {
		const filter: TagFilter = { all: ['32'], any: ['31', '30'] };
		const sorted = withSort(withFilter(url, filter), {
			field: 'title',
			order: 'asc'
		});
		expect(readFilter(sorted.searchParams)).toEqual(filter);
	});

	it('takes the filter out when unfiltered', () => {
		const filtered = withFilter(url, { all: ['30'], any: ['31'] });
		expect(withFilter(filtered, UNFILTERED).href).toBe(url.href);
	});
});

describe("this page's filter", () => {
	it('is what the URL holds', () => {
		at.url = new URL('https://jewelcase.test/albums?tagAny=31');
		expect(currentFilter()).toEqual({ all: [], any: ['31'] });
	});

	it('changes in place: the same history entry, focus, and scroll', async () => {
		at.url = new URL('https://jewelcase.test/songs?sort=title&order=asc');
		await showFiltered({ all: [], any: ['30'] });
		expect(goto).toHaveBeenLastCalledWith(
			new URL('https://jewelcase.test/songs?sort=title&order=asc&tagAny=30'),
			{ replaceState: true, keepFocus: true, noScroll: true }
		);
	});

	it('shows a filter just chosen until its load finishes, or fails', async () => {
		at.url = new URL('https://jewelcase.test/songs');
		const loads: { done: () => void; fail: () => void }[] = [];
		vi.mocked(goto).mockImplementation(
			() =>
				new Promise<void>((done, fail) =>
					loads.push({ done, fail: () => fail(new Error('aborted')) })
				)
		);
		const rock: TagFilter = { all: [], any: ['30'] };
		const both: TagFilter = { all: [], any: ['30', '31'] };
		const first = showFiltered(rock);
		expect(shownFilter(UNFILTERED)).toBe(rock);
		const second = showFiltered(both);
		// The first load finishing leaves the later choice showing.
		loads[0].done();
		await first;
		expect(shownFilter(rock)).toBe(both);
		// Elsewhere, it is not this page's.
		at.url = new URL('https://jewelcase.test/albums/1');
		expect(shownFilter(UNFILTERED)).toBe(UNFILTERED);
		at.url = new URL('https://jewelcase.test/songs');
		loads[1].fail();
		await expect(second).rejects.toThrow('aborted');
		expect(shownFilter(rock)).toBe(rock);
		vi.mocked(goto).mockImplementation(async () => {});
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
	// Rock is 30, Jazz 31, Blues 33, Calm 32 and Pop 34, which no item carries.
	const items = [
		item('daybreak', ['30'], ['32']),
		item('static', ['30']),
		item('drift', ['31'], ['32']),
		item('pulse', ['30', '31']),
		item('ember', ['30', '33'])
	];
	const kept = (all: string[], any: string[], typeOf?: (tag: string) => string) =>
		filterByTags(items, { all, any }, (i) => i.tags, typeOf).map((i) => i.name);

	it('keeps everything with no filter', () => {
		expect(kept([], [])).toEqual(['daybreak', 'static', 'drift', 'pulse', 'ember']);
	});

	it('matches any of one type, or all', () => {
		expect(kept([], ['30', '31'])).toEqual(['daybreak', 'static', 'drift', 'pulse', 'ember']);
		expect(kept(['30', '31'], [])).toEqual(['pulse']);
	});

	it('matches every type named, and all and any of one type together', () => {
		expect(kept([], ['30', '32'])).toEqual(['daybreak']);
		expect(kept([], ['31', '30', '32'])).toEqual(['daybreak', 'drift']);
		// Rock, and Jazz or Blues.
		expect(kept(['30'], ['31', '33'])).toEqual(['pulse', 'ember']);
	});

	it('groups a tag no item carries by the type it is known to have', () => {
		const genre = () => 'genre';
		expect(kept([], ['30', '34'], genre)).toEqual(['daybreak', 'static', 'pulse', 'ember']);
		expect(kept(['30', '34'], [], genre)).toEqual([]);
		// Not known, or not the library's, it matches nothing.
		expect(kept([], ['30', '34'])).toEqual([]);
	});

	it('says whether every any tag has a type to group by', () => {
		/** A `known` that says only `ids` have their type read. */
		const known = (ids: string[]) => (tag: string) => ids.includes(tag);
		/** Whether the tags in `any` all have a type, given the types read for `ids`. */
		const types = (any: string[], ids: string[] = []) =>
			tagTypesKnown(items, { all: [], any }, (i) => i.tags, known(ids));
		expect(types([])).toBe(true);
		// Carried by an item: rock and jazz.
		expect(types(['30', '31'])).toBe(true);
		// Pop is carried by none, so its type must have been read.
		expect(types(['30', '34'])).toBe(false);
		expect(types(['30', '34'], ['34'])).toBe(true);
		// A tag in `all` needs no type.
		expect(tagTypesKnown(items, { all: ['34'], any: [] }, (i) => i.tags, known([]))).toBe(true);
	});
});
