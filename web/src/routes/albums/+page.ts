import { listAlbums } from '$lib/api/client';
import { filterQuery, readFilter } from '$lib/filters';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	// The server filters, so a new filter loads again (`requirements/tags.md` §7).
	const filter = readFilter(url.searchParams);
	const query = { sort: 'name', ...filterQuery(filter) };
	return { lib, filter, query, albums: await listAlbums(fetch, lib, query) };
};
