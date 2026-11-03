// A list's tag filter lives in its URL in the API's own terms, `?tagAll=30&tagAny=31,32`, so a
// reload, Back, or a shared link returns to the list as it was filtered, and a page hands its
// URL's filter to the API as it is (`requirements/tags.md` §7). Each tag type's picker chooses
// whether its tags go in `tagAll` or `tagAny`.

import { goto } from '$app/navigation';
import { page } from '$app/state';
import { choose, pendingFilter, settle } from './pending-filter.svelte';
import { type TagFilter, type TagMatch, chosenTags } from './tag-match';

export {
	type TagFilter,
	type TagMatch,
	UNFILTERED,
	chosenTags,
	filterByTags,
	isFiltered,
	tagTypesKnown
} from './tag-match';

/** The filter `params` hold: each list's tags once, in the order chosen. */
export function readFilter(params: URLSearchParams): TagFilter {
	const ids = (key: string) => [
		...new Set((params.get(key) ?? '').split(',').filter((tag) => tag !== ''))
	];
	return { all: ids('tagAll'), any: ids('tagAny') };
}

/**
 * How `filter` matches `tags`, one type's chosen tags: `all` where every one is in `filter.all`,
 * `any` where every one is in `filter.any`, and `undefined` for none, or a mix, which only a URL
 * written by hand holds.
 */
export function typeMatch(filter: TagFilter, tags: readonly string[]): TagMatch | undefined {
	if (tags.length === 0) return undefined;
	if (tags.every((tag) => filter.all.includes(tag) && !filter.any.includes(tag))) return 'all';
	if (tags.every((tag) => filter.any.includes(tag) && !filter.all.includes(tag))) return 'any';
	return undefined;
}

/** `filter` without `tags`. */
export function withoutTags(filter: TagFilter, tags: readonly string[]): TagFilter {
	const kept = (list: string[]) => list.filter((tag) => !tags.includes(tag));
	return { all: kept(filter.all), any: kept(filter.any) };
}

/**
 * `filter` with `tag` taken out if it was chosen, and otherwise added to the list its type's
 * chosen tags, `typeTags`, are in: `all` if they are all there, `any` otherwise.
 */
export function toggleTag(filter: TagFilter, tag: string, typeTags: readonly string[]): TagFilter {
	if (chosenTags(filter).includes(tag)) return withoutTags(filter, [tag]);
	return typeMatch(filter, typeTags) === 'all'
		? { ...filter, all: [...filter.all, tag] }
		: { ...filter, any: [...filter.any, tag] };
}

/** `filter` with `typeTags`, one type's chosen tags, all moved to `match`'s list, in their order. */
export function matchTags(
	filter: TagFilter,
	typeTags: readonly string[],
	match: TagMatch
): TagFilter {
	const rest = withoutTags(filter, typeTags);
	return match === 'all'
		? { ...rest, all: [...rest.all, ...typeTags] }
		: { ...rest, any: [...rest.any, ...typeTags] };
}

/**
 * The API's parameters for `filter`, to spread into a list's or a search's query: only the lists
 * that name tags.
 */
export function filterQuery(filter: TagFilter): { tagAll?: string; tagAny?: string } {
	return {
		...(filter.all.length && { tagAll: filter.all.join(',') }),
		...(filter.any.length && { tagAny: filter.any.join(',') })
	};
}

/** `url` with `filter` in place of the filter it had, and everything else, its sort included, as it was. */
export function withFilter(url: URL, filter: TagFilter): URL {
	const next = new URL(url);
	next.searchParams.delete('tagAll');
	next.searchParams.delete('tagAny');
	for (const [key, value] of Object.entries(filterQuery(filter))) next.searchParams.set(key, value);
	return next;
}

/** This page's filter. Read in a `$derived`, it follows the URL. */
export function currentFilter(): TagFilter {
	return readFilter(page.url.searchParams);
}

/**
 * `loaded`, the filter this page last loaded, or the one chosen on it since, while its load has
 * not finished. Every filter control shows this one, and builds the next change on it.
 */
export function shownFilter(loaded: TagFilter): TagFilter {
	const pending = pendingFilter();
	return pending && pending.path === page.url.pathname ? pending.filter : loaded;
}

/**
 * Shows this page filtered by `filter`. As with a sort, each change replaces the history entry,
 * so Back leaves the list rather than stepping back through every tag chosen; a page whose load
 * reads the filter loads again. Until it has, [`shownFilter`] is `filter`.
 */
export async function showFiltered(filter: TagFilter): Promise<void> {
	choose(page.url.pathname, filter);
	try {
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- this page's own URL, already resolved
		await goto(withFilter(page.url, filter), {
			replaceState: true,
			keepFocus: true,
			noScroll: true
		});
	} finally {
		settle(filter);
	}
}
