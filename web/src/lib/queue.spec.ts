import { describe, expect, it } from 'vitest';
import {
	albumRecipe,
	artistRecipe,
	firstPlayable,
	libraryRecipe,
	playlistRecipe,
	recipeFilters,
	restartsOnPrevious,
	searchRecipe
} from './queue';

const ok = { availability: 'available' as const };
const gone = { availability: 'missing' as const };

describe('firstPlayable', () => {
	it('starts on the chosen track, or the next that can play', () => {
		expect(firstPlayable([ok, ok], 1)).toBe(1);
		expect(firstPlayable([ok, gone, ok], 1)).toBe(2);
		expect(firstPlayable([gone], 0)).toBeNull();
		expect(firstPlayable([ok, gone], 1)).toBeNull();
	});
});

describe('restartsOnPrevious', () => {
	it('restarts a track that is under way', () => {
		expect(restartsOnPrevious(12, 5)).toBe(true);
	});

	it('steps back near the start of a track', () => {
		expect(restartsOnPrevious(1, 5)).toBe(false);
		expect(restartsOnPrevious(3, 1)).toBe(false);
	});

	it('restarts when nothing has played before it', () => {
		expect(restartsOnPrevious(0, 0)).toBe(true);
	});
});

describe('recipes', () => {
	it('take the sort and order as listTracks does, in album order unless told', () => {
		expect(albumRecipe('1')).toEqual({ type: 'album', albumId: '1', sort: 'album', order: 'asc' });
		expect(artistRecipe('2', { sort: 'releaseDate', order: 'desc' })).toEqual({
			type: 'artist',
			artistId: '2',
			scope: 'all',
			artistCredit: 'any',
			sort: 'releaseDate',
			order: 'desc'
		});
	});
});

describe('recipes with a tag filter', () => {
	const filter = { all: ['30', '32'], any: ['61'] };
	const none = { all: [], any: [] };

	it('name only the lists that hold tags', () => {
		expect(recipeFilters(filter)).toEqual({ tagAll: ['30', '32'], tagAny: ['61'] });
		expect(recipeFilters({ all: [], any: ['61'] })).toEqual({ tagAny: ['61'] });
		expect(recipeFilters(none)).toEqual({});
	});

	it('carry the filter on album, library and search recipes', () => {
		const sort = { sort: 'name', order: 'desc' } as const;
		expect(albumRecipe('1', undefined, filter)).toEqual({
			type: 'album',
			albumId: '1',
			filters: { tagAll: ['30', '32'], tagAny: ['61'] },
			sort: 'album',
			order: 'asc'
		});
		expect(libraryRecipe(sort, filter)).toEqual({
			type: 'library',
			filters: { tagAll: ['30', '32'], tagAny: ['61'] },
			...sort
		});
		expect(searchRecipe('sig', filter)).toEqual({
			type: 'search',
			q: 'sig',
			filters: { tagAll: ['30', '32'], tagAny: ['61'] }
		});
	});

	it('name a playlist, with sort and order together and its filter, or its own order', () => {
		expect(playlistRecipe('9')).toEqual({ type: 'playlist', playlistId: '9' });
		expect(playlistRecipe('9', null, none)).toEqual({ type: 'playlist', playlistId: '9' });
		expect(playlistRecipe('9', { field: 'added', order: 'desc' }, filter)).toEqual({
			type: 'playlist',
			playlistId: '9',
			filters: { tagAll: ['30', '32'], tagAny: ['61'] },
			sort: 'addedToPlaylist',
			order: 'desc'
		});
	});

	it('leave out an empty filter, except a library recipe, which always has its filters', () => {
		expect(albumRecipe('1', undefined, none)).not.toHaveProperty('filters');
		expect(searchRecipe('sig', none)).toEqual({ type: 'search', q: 'sig' });
		expect(libraryRecipe({ sort: 'name', order: 'asc' }, none)).toHaveProperty('filters', {});
	});
});
