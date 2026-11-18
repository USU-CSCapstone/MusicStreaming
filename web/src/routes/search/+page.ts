import { search } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, url }) => {
	const lib = await requireLibrary(parent);
	const q = url.searchParams.get('q') ?? '';
	// Search is not filtered by tag: tags are found by name instead (`requirements/search.md` §5).
	const query = { sectionLimit: 12 };
	return { lib, q, results: q.trim() ? await search(fetch, lib, q, query) : null };
};
