// The tabs above a search's results: All, then one for each type of result, as the sidebar
// names and orders them, a type of tag each after the rest, and Lyrics last, which the sidebar
// has no place for (`requirements/search.md` §4–5). A tab is in the URL, so Back and a link keep
// it.

import { resolve } from '$app/paths';
import type { SearchSectionType, TagType } from './api/types';
import { tagTypePlural } from './tags';

/** A type of result a tab shows: any a search finds. */
export type SearchTab = SearchSectionType;

/** A tab that shows one section: one type of result, and for tags one type of tag. */
export type Tab = { type: SearchTab; tagType?: string };

/** The query parameters that name the tab; without them, All. */
export const TAB_PARAM = 'type';
export const TAG_TYPE_PARAM = 'tagType';

/** Each section's tab but the tags', in the sidebar's order, with the sidebar's name for it. */
export const SEARCH_TABS: { type: Exclude<SearchTab, 'tags' | 'lyrics'>; label: string }[] = [
	{ type: 'albums', label: 'Albums' },
	{ type: 'artists', label: 'Artists' },
	{ type: 'playlists', label: 'Playlists' },
	{ type: 'tracks', label: 'Songs' }
];

/** The tab of lyric matches, after every other. */
export const LYRICS_TAB = { type: 'lyrics', label: 'Lyrics' } as const;

/** Each tab of one type, whatever its place, with its name. */
const NAMED: { type: Exclude<SearchTab, 'tags'>; label: string }[] = [...SEARCH_TABS, LYRICS_TAB];

/**
 * Every tab, in order, with its name: All, each section's, then one for each type of tag the
 * library has, `types` as `listTagTypes` gives them, then Lyrics.
 */
export function searchTabs(types: readonly TagType[]): { tab: Tab | null; label: string }[] {
	return [
		{ tab: null, label: 'All' },
		...SEARCH_TABS.map(({ type, label }) => ({ tab: { type }, label })),
		...types.map((tagType) => ({
			tab: { type: 'tags' as const, tagType: tagType.key },
			label: tagTypePlural(tagType.key, types)
		})),
		{ tab: { type: LYRICS_TAB.type }, label: LYRICS_TAB.label }
	];
}

/** The name of `tab`, as its link says it, or null for All. */
export function tabLabel(tab: Tab | null, types: readonly TagType[]): string | null {
	if (!tab) return null;
	if (tab.type === 'tags') return tagTypePlural(tab.tagType ?? '', types);
	return NAMED.find((t) => t.type === tab.type)?.label ?? null;
}

/** The tab `params` names, or null for All, as for a tab that is none of these. */
export function readTab(params: URLSearchParams): Tab | null {
	const type = params.get(TAB_PARAM);
	if (type === 'tags') {
		const tagType = params.get(TAG_TYPE_PARAM);
		return tagType ? { type, tagType } : null;
	}
	const found = NAMED.find((tab) => tab.type === type);
	return found ? { type: found.type } : null;
}

/** Whether `a` and `b` are the same tab. */
export const sameTab = (a: Tab | null, b: Tab | null) =>
	a?.type === b?.type && a?.tagType === b?.tagType;

/** The query parameters that name `tab`. */
export function tabParams(tab: Tab | null): Record<string, string> {
	if (!tab) return {};
	return { [TAB_PARAM]: tab.type, ...(tab.tagType && { [TAG_TYPE_PARAM]: tab.tagType }) };
}

/** The search for `q` on `tab`, or on All, with `scope`'s parameters (`search-scope.ts`). */
export function tabHref(q: string, tab: Tab | null, scope: Record<string, string> = {}): string {
	return `${resolve('/search')}?${new URLSearchParams({ q, ...tabParams(tab), ...scope })}`;
}
