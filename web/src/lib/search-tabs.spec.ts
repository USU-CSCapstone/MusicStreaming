import { describe, expect, it } from 'vitest';
import { SEARCH_TABS, readTab, tabHref } from './search-tabs';

describe('search tabs', () => {
	it('come in the sidebar’s order, named as it names them', () => {
		expect(SEARCH_TABS.map((tab) => tab.label)).toEqual(['Albums', 'Artists', 'Playlists', 'Songs']);
	});

	it('are read from the URL, anything else being All', () => {
		expect(readTab(new URLSearchParams('q=blue&type=albums'))).toBe('albums');
		expect(readTab(new URLSearchParams('q=blue&type=tracks'))).toBe('tracks');
		expect(readTab(new URLSearchParams('q=blue'))).toBeNull();
		expect(readTab(new URLSearchParams('q=blue&type=lyrics'))).toBeNull();
		expect(readTab(new URLSearchParams('q=blue&type=nope'))).toBeNull();
	});

	it('link to the same search on another tab', () => {
		expect(tabHref("say it ain't so", 'tracks')).toBe('/search?q=say+it+ain%27t+so&type=tracks');
		expect(tabHref('blue', null)).toBe('/search?q=blue');
	});
});
