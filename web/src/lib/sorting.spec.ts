import { describe, expect, it } from 'vitest';
import { nextSort, serverSort, sortName } from './sorting';

describe('nextSort', () => {
	it('goes ascending, descending, then unsorted', () => {
		const asc = nextSort(null, 'artist');
		expect(asc).toEqual({ field: 'artist', order: 'asc' });
		const desc = nextSort(asc, 'artist');
		expect(desc).toEqual({ field: 'artist', order: 'desc' });
		expect(nextSort(desc, 'artist')).toBeNull();
	});

	it('starts another column over at ascending', () => {
		expect(nextSort({ field: 'artist', order: 'desc' }, 'year')).toEqual({
			field: 'year',
			order: 'asc'
		});
	});
});

describe('serverSort', () => {
	it('names each sort as listTracks does', () => {
		expect(serverSort({ field: 'title', order: 'asc' })).toEqual({ sort: 'name', order: 'asc' });
		expect(serverSort({ field: 'year', order: 'desc' })).toEqual({
			sort: 'releaseDate',
			order: 'desc'
		});
		expect(serverSort({ field: 'duration', order: 'asc' })?.sort).toBe('duration');
	});

	it('leaves unsorted, and date added to a playlist, to the list', () => {
		expect(serverSort(null)).toBeNull();
		expect(serverSort({ field: 'added', order: 'asc' })).toBeNull();
	});
});

describe('sortName', () => {
	it("names a playlist's date added as the core and a playlist recipe do", () => {
		expect(sortName('added')).toBe('addedToPlaylist');
		expect(sortName('title')).toBe('name');
	});
});
