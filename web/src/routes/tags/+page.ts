import { listTags } from '$lib/api/client';
import { catalog } from '$lib/catalog/client.svelte';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	// Taken before reading, so the page can tell a change announced while it loads.
	const since = catalog.changes;
	// One type's tags, by name; a type the library has none of lists none.
	const type = url.searchParams.get('type') ?? 'genre';
	const query = { type, sort: 'name' };
	return { lib, since, type, query, tags: await listTags(fetch, lib, query) };
};
