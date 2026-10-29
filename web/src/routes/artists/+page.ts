import { listArtists } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent }) => {
	const lib = await requireLibrary(parent);
	return { lib, artists: await listArtists(fetch, lib, { sort: 'name' }) };
};
