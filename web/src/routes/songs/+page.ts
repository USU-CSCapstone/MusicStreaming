import { listTracks } from '$lib/api/client';
import { filterQuery, readFilter } from '$lib/filters';
import { requireLibrary } from '$lib/library';
import { readSort } from '$lib/sort-url';
import { FIELDS, serverSort } from '$lib/sorting';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	// The whole library is too big to hold, so the server sorts and filters it, and a new sort or
	// filter loads again. Unsorted, it shows what was added most recently
	// (`requirements/conventions.md` §4.1).
	const sort = readSort(url.searchParams, FIELDS);
	const filter = readFilter(url.searchParams);
	const query = {
		...(serverSort(sort) ?? { sort: 'dateAdded', order: 'desc' }),
		...filterQuery(filter)
	};
	return { lib, sort, filter, query, tracks: await listTracks(fetch, lib, query) };
};
