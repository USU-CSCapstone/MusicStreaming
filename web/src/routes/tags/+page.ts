import { listTags } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	// One type's tags, by name; a type the library has none of lists none.
	const type = url.searchParams.get('type') ?? 'genre';
	const query = { type, sort: 'name' };
	return { lib, type, query, tags: await listTags(fetch, lib, query) };
};
