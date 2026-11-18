import { search, searchSection } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import { readTab } from '$lib/search-tabs';
import { readSort } from '$lib/sort-url';
import { FIELDS, serverSort } from '$lib/sorting';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	const q = url.searchParams.get('q') ?? '';
	const tab = readTab(url.searchParams);
	if (!q.trim()) return { lib, q, tab, results: null, songs: null };
	// The Songs tab is every song found, a page at a time, in the search's ranking or sorted by the
	// server, which holds them all (`requirements/search.md` §4).
	if (tab === 'tracks') {
		const query = serverSort(readSort(url.searchParams, FIELDS)) ?? {};
		const first = await searchSection(fetch, lib, 'tracks', q, query);
		return { lib, q, tab, results: null, songs: { query, first } };
	}
	// All shows each section's best; another tab, more of its own. Search is not filtered by tag:
	// tags are found by name instead (`requirements/search.md` §5).
	const query = tab ? { types: tab, sectionLimit: 50 } : { sectionLimit: 12 };
	return { lib, q, tab, results: await search(fetch, lib, q, query), songs: null };
};
