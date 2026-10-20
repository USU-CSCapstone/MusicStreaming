import { listTracks } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import { readSort } from '$lib/sort-url';
import { FIELDS, serverSort } from '$lib/sorting';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	// The whole library is too big to hold, so the server sorts it, and a new sort loads again.
	// Unsorted, it shows what was added most recently (`requirements/conventions.md` §4.1).
	const sort = readSort(url.searchParams, FIELDS);
	const query = serverSort(sort) ?? { sort: 'dateAdded', order: 'desc' };
	return { lib, sort, query, tracks: await listTracks(fetch, lib, query) };
};
