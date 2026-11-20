import { search, searchSection } from '$lib/api/client';
import { catalog } from '$lib/catalog/client.svelte';
import { requireLibrary } from '$lib/library';
import { readTab } from '$lib/search-tabs';
import { readSort } from '$lib/sort-url';
import { FIELDS, serverSort } from '$lib/sorting';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	const q = url.searchParams.get('q') ?? '';
	const tab = readTab(url.searchParams);
	// Taken before reading, so the page can tell a change announced while it loads.
	const since = catalog.changes;
	if (!q.trim()) return { lib, q, tab, since, results: null, section: null };
	// A tab is every match of its type, a page at a time, in the search's ranking; the Songs tab's
	// may be sorted instead, by the server, which holds them all (`requirements/search.md` §4).
	if (tab) {
		const query = tab === 'tracks' ? (serverSort(readSort(url.searchParams, FIELDS)) ?? {}) : {};
		const first = await searchSection(fetch, lib, tab, q, query);
		return { lib, q, tab, since, results: null, section: { query, first } };
	}
	// All shows each section's best. Search is not filtered by tag: tags are found by name
	// instead (`requirements/search.md` §5).
	return {
		lib,
		q,
		tab,
		since,
		results: await search(fetch, lib, q, { sectionLimit: 12 }),
		section: null
	};
};
