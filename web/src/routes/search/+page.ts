import { search } from '$lib/api/client';
import { filterQuery, readFilter } from '$lib/filters';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	const q = url.searchParams.get('q') ?? '';
	// The server narrows a search by tags before it ranks and counts, which a page of the best
	// matches could not do on its own (`requirements/search.md` §5).
	const filter = readFilter(url.searchParams);
	const query = { sectionLimit: 12, ...filterQuery(filter) };
	return { lib, q, filter, results: q.trim() ? await search(fetch, lib, q, query) : null };
};
