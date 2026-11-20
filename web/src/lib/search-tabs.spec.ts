import { describe, expect, it } from 'vitest';
import type { TagType } from './api/types';
import { SEARCH_TABS, readTab, sameTab, searchTabs, tabHref, tabLabel } from './search-tabs';

const types: TagType[] = [
	{ key: 'genre', tagCount: 12 },
	{ key: 'mood', tagCount: 3 },
	{ key: 'decade', name: 'Decade', tagCount: 5 }
];

describe('search tabs', () => {
	it('come in the sidebar’s order, named as it names them, a type of tag each after the rest', () => {
		expect(SEARCH_TABS.map((tab) => tab.label)).toEqual([
			'Albums',
			'Artists',
			'Playlists',
			'Songs'
		]);
		expect(searchTabs(types).map(({ label }) => label)).toEqual([
			'All',
			'Albums',
			'Artists',
			'Playlists',
			'Songs',
			'Genres',
			'Moods',
			'Decade',
			'Lyrics'
		]);
		expect(searchTabs([]).map(({ label }) => label).slice(-2)).toEqual(['Songs', 'Lyrics']);
	});

	it('are read from the URL, anything else being All', () => {
		expect(readTab(new URLSearchParams('q=blue&type=albums'))).toEqual({ type: 'albums' });
		expect(readTab(new URLSearchParams('q=blue&type=tags&tagType=mood'))).toEqual({
			type: 'tags',
			tagType: 'mood'
		});
		expect(readTab(new URLSearchParams('q=blue'))).toBeNull();
		expect(readTab(new URLSearchParams('q=blue&type=tags'))).toBeNull();
		expect(readTab(new URLSearchParams('q=blue&type=lyrics'))).toEqual({ type: 'lyrics' });
		expect(readTab(new URLSearchParams('q=blue&type=nope'))).toBeNull();
	});

	it('link to the same search on another tab', () => {
		expect(tabHref("say it ain't so", { type: 'tracks' })).toBe(
			'/search?q=say+it+ain%27t+so&type=tracks'
		);
		expect(tabHref('rock', { type: 'tags', tagType: 'genre' })).toBe(
			'/search?q=rock&type=tags&tagType=genre'
		);
		expect(tabHref('blue', null)).toBe('/search?q=blue');
	});

	it('are named, and told apart by their type of tag', () => {
		expect(tabLabel({ type: 'tracks' }, types)).toBe('Songs');
		expect(tabLabel({ type: 'lyrics' }, types)).toBe('Lyrics');
		expect(tabLabel({ type: 'tags', tagType: 'mood' }, types)).toBe('Moods');
		expect(tabLabel(null, types)).toBeNull();
		expect(sameTab({ type: 'tags', tagType: 'mood' }, { type: 'tags', tagType: 'mood' })).toBe(
			true
		);
		expect(sameTab({ type: 'tags', tagType: 'mood' }, { type: 'tags', tagType: 'genre' })).toBe(
			false
		);
		expect(sameTab(null, null)).toBe(true);
	});
});
