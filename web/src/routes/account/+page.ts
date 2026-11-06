import { listHiddenTracks, listMyPlugins } from '$lib/api/client';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent }) => {
	const { library } = await parent();
	// Hidden Songs is a part of the page; one that fails to load leaves it out, not the page.
	const [{ items }, hidden] = await Promise.all([
		listMyPlugins(fetch),
		library ? listHiddenTracks(fetch, library.id).catch(() => null) : null
	]);
	return { connectable: items, hidden };
};
