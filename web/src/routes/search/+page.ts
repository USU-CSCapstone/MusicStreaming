import { search, searchSection } from '$lib/api/client';
import { catalog } from '$lib/catalog/client.svelte';
import { requireLibrary } from '$lib/library';
import { FROM_PARAM, HELD, WITHIN_PARAM, readContainer } from '$lib/search-scope';
import { readTab } from '$lib/search-tabs';
import { readSort } from '$lib/sort-url';
import { FIELDS, serverSort } from '$lib/sorting';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	// Taken before reading, so the page can tell a change announced while it loads.
	const since = catalog.changes;
	const params = url.searchParams;
	const q = params.get('q') ?? '';
	// The container the search was started from, offered or kept to, by its name; one gone, or
	// out of the caller's reach, is no longer offered (`requirements/search.md` §5).
	const from = params.get(FROM_PARAM);
	const container = from ? await readContainer(fetch, lib, from) : null;
	const within = container !== null && params.get(WITHIN_PARAM) === container.within;
	const narrowed = within ? { within: container.within } : {};
	// Kept to a container, a tab of what it cannot hold is All.
	const read = readTab(params);
	const tab = read && (!within || HELD[container.kind].includes(read.type)) ? read : null;
	if (!q.trim()) return { lib, q, tab, container, within, since, results: null, section: null };
	// A tab is every match of its type, a page at a time, in the search's ranking; the Songs tab's
	// may be sorted instead, by the server, which holds them all, and a tags tab is one type of
	// tag (`requirements/search.md` §4).
	if (tab) {
		const query = {
			...(tab.type === 'tracks'
				? (serverSort(readSort(params, FIELDS)) ?? {})
				: { tagType: tab.tagType }),
			...narrowed
		};
		const first = await searchSection(fetch, lib, tab.type, q, query);
		return { lib, q, tab, container, within, since, results: null, section: { query, first } };
	}
	// All shows each section's best. Search is not filtered by tag: tags are found by name
	// instead (`requirements/search.md` §5).
	const results = await search(fetch, lib, q, { sectionLimit: 12, ...narrowed });
	return { lib, q, tab, container, within, since, results, section: null };
};
