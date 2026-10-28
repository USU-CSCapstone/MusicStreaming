// A list's tag filter lives in its URL in the API's own terms, `?tag=30,32&tagMatch=all`, so a
// reload, Back, or a shared link returns to the list as it was filtered, and a page hands its
// URL's filter to the API as it is (`requirements/tags.md` §7).

import { goto } from '$app/navigation';
import { page } from '$app/state';

/** Whether an item must carry all or any of the chosen tags of each type (`api/openapi.yaml`). */
export type TagMatch = 'any' | 'all';

/**
 * The tags a list is narrowed to, by ID, in the order they were chosen. The server groups them
 * by type: within one type `match` decides, and every type named must match, since types are
 * independent (`requirements/tags.md` §2).
 */
export type TagFilter = { tags: string[]; match: TagMatch };

/** No filter. */
export const UNFILTERED: TagFilter = { tags: [], match: 'any' };

/** The filter `params` hold: each tag once, and `any` unless they say `all`. */
export function readFilter(params: URLSearchParams): TagFilter {
	const named = (params.get('tag') ?? '').split(',').filter((tag) => tag !== '');
	return {
		tags: [...new Set(named)],
		match: params.get('tagMatch') === 'all' ? 'all' : 'any'
	};
}

/** Whether `filter` narrows a list at all. */
export function isFiltered(filter: TagFilter): boolean {
	return filter.tags.length > 0;
}

/** `filter` with `tag` added if it was not chosen, and taken out if it was. */
export function toggleTag(filter: TagFilter, tag: string): TagFilter {
	const tags = filter.tags.includes(tag)
		? filter.tags.filter((chosen) => chosen !== tag)
		: [...filter.tags, tag];
	return { ...filter, tags };
}

/**
 * The API's parameters for `filter`, to spread into a list's or a search's query: none for no
 * filter, and `tagMatch` only where it is not the API's default.
 */
export function filterQuery(filter: TagFilter): {
	tag?: string;
	tagMatch?: TagMatch;
} {
	if (!isFiltered(filter)) return {};
	return filter.match === 'all'
		? { tag: filter.tags.join(','), tagMatch: 'all' }
		: { tag: filter.tags.join(',') };
}

/** `url` with `filter` in place of the filter it had, and everything else, its sort included, as it was. */
export function withFilter(url: URL, filter: TagFilter): URL {
	const next = new URL(url);
	next.searchParams.delete('tag');
	next.searchParams.delete('tagMatch');
	for (const [key, value] of Object.entries(filterQuery(filter))) next.searchParams.set(key, value);
	return next;
}

/** This page's filter. Read in a `$derived`, it follows the URL. */
export function currentFilter(): TagFilter {
	return readFilter(page.url.searchParams);
}

/**
 * Shows this page filtered by `filter`. As with a sort, each change replaces the history entry,
 * so Back leaves the list rather than stepping back through every tag chosen; a page whose load
 * reads the filter loads again.
 */
export function showFiltered(filter: TagFilter): Promise<void> {
	// eslint-disable-next-line svelte/no-navigation-without-resolve -- this page's own URL, already resolved
	return goto(withFilter(page.url, filter), {
		replaceState: true,
		keepFocus: true,
		noScroll: true
	});
}
