import { describe, expect, it } from 'vitest';
import { readSort, withSort } from './sort-url';
import { FIELDS, PLAYLIST_FIELDS } from './sorting';

const params = (query: string) => new URLSearchParams(query);

describe('readSort', () => {
	it('reads the field and order', () => {
		expect(readSort(params('sort=artist&order=desc'), FIELDS)).toEqual({
			field: 'artist',
			order: 'desc'
		});
	});

	it('is ascending unless the order says descending', () => {
		expect(readSort(params('sort=year'), FIELDS)).toEqual({ field: 'year', order: 'asc' });
		expect(readSort(params('sort=year&order=sideways'), FIELDS)?.order).toBe('asc');
	});

	it("is unsorted for a sort this list doesn't offer", () => {
		expect(readSort(params(''), FIELDS)).toBeNull();
		expect(readSort(params('sort=plays'), FIELDS)).toBeNull();
		// Date added is a playlist's alone.
		expect(readSort(params('sort=added'), FIELDS)).toBeNull();
		expect(readSort(params('sort=added'), PLAYLIST_FIELDS)).toEqual({
			field: 'added',
			order: 'asc'
		});
	});
});

describe('withSort', () => {
	const url = new URL('https://jewelcase.test/search?q=signal#top');

	it('sets the sort and keeps the rest of the URL', () => {
		const sorted = withSort(url, { field: 'title', order: 'desc' });
		expect(sorted.href).toBe('https://jewelcase.test/search?q=signal&sort=title&order=desc#top');
		expect(url.href).toBe('https://jewelcase.test/search?q=signal#top');
	});

	it('round-trips through readSort', () => {
		const sort = { field: 'duration', order: 'asc' } as const;
		expect(readSort(withSort(url, sort).searchParams, FIELDS)).toEqual(sort);
	});

	it('takes the sort out when unsorted', () => {
		const sorted = withSort(url, { field: 'album', order: 'asc' });
		expect(withSort(sorted, null).href).toBe(url.href);
	});
});
