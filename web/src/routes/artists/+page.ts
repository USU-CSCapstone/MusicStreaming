import { listArtists } from '$lib/api/client';
import { catalog } from '$lib/catalog/client.svelte';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent }) => {
	const lib = await requireLibrary(parent);
	// Taken before reading, so the page can tell a change announced while it loads.
	const since = catalog.changes;
	return { lib, since, artists: await listArtists(fetch, lib, { sort: 'name' }) };
};
