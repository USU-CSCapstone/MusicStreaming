// The tabs above a search's results: All, then one for each type of result, as the sidebar
// names and orders them (`requirements/search.md` §4–5). A tab is in the URL, so Back and a
// link keep it.

import { resolve } from '$app/paths';
import type { SearchSectionType } from './api/types';

/** A tab that shows one section: the types of result a search finds, but lyrics, not yet searched. */
export type SearchTab = Exclude<SearchSectionType, 'lyrics'>;

/** The query parameter that names the tab; without it, All. */
export const TAB_PARAM = 'type';

/** Each section's tab, in the sidebar's order, with the sidebar's name for it. */
export const SEARCH_TABS: { type: SearchTab; label: string }[] = [
	{ type: 'albums', label: 'Albums' },
	{ type: 'artists', label: 'Artists' },
	{ type: 'playlists', label: 'Playlists' },
	{ type: 'tracks', label: 'Songs' }
];

/** The tab `params` names, or null for All, as for a tab that is none of these. */
export function readTab(params: URLSearchParams): SearchTab | null {
	const type = params.get(TAB_PARAM);
	return SEARCH_TABS.find((tab) => tab.type === type)?.type ?? null;
}

/** The search for `q` on `tab`, or on All. */
export function tabHref(q: string, tab: SearchTab | null): string {
	const query = new URLSearchParams({ q, ...(tab && { [TAB_PARAM]: tab }) });
	return `${resolve('/search')}?${query}`;
}
