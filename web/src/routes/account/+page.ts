import { listHiddenTracks, listMyPlugins } from '$lib/api/client';
import { unreachable } from '$lib/catalog/read';
import type { PageLoad } from './$types';

/**
 * Loads the plugins the user can connect, or null when the server cannot be reached, and how
 * many songs are hidden in the library on screen, or null when that cannot be read.
 */
export const load: PageLoad = async ({ fetch, parent }) => {
	const { library } = await parent();
	// The plugins the server offers need it; without it the page still opens, to turn Work offline
	// off. The hidden count is for the link to Hidden Songs: one that fails leaves it out, not the
	// page.
	const [connectable, hidden] = await Promise.all([
		listMyPlugins(fetch).then(
			({ items }) => items,
			(error) => {
				if (unreachable(error)) return null;
				throw error;
			}
		),
		library ? listHiddenTracks(fetch, library.id, { limit: 1 }).catch(() => null) : null
	]);
	return { connectable, hiddenCount: hidden?.total ?? null };
};
