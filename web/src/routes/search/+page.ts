import { search } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	const q = url.searchParams.get('q') ?? '';
	return { q, results: q.trim() ? await search(fetch, lib, q, { sectionLimit: 12 }) : null };
};
