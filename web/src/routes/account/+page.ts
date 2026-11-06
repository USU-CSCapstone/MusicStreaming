import { listHiddenTracks, listMyPlugins } from '$lib/api/client';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent }) => {
	const { library } = await parent();
	// How many songs are hidden, for the link to them; one that fails to load leaves the count out,
	// not the page.
	const [{ items }, hidden] = await Promise.all([
		listMyPlugins(fetch),
		library ? listHiddenTracks(fetch, library.id, { limit: 1 }).catch(() => null) : null
	]);
	return { connectable: items, hiddenCount: hidden?.total ?? null };
};
