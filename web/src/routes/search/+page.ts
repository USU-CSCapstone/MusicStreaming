import { search } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import { readTab } from '$lib/search-tabs';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	const q = url.searchParams.get('q') ?? '';
	// All shows each section's best; a tab, more of its own (`requirements/search.md` §4–5).
	// Search is not filtered by tag: tags are found by name instead.
	const tab = readTab(url.searchParams);
	const query = tab ? { types: tab, sectionLimit: 50 } : { sectionLimit: 12 };
	return { lib, q, tab, results: q.trim() ? await search(fetch, lib, q, query) : null };
};
