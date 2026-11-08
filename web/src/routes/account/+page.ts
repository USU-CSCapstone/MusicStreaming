import { listMyPlugins } from '$lib/api/client';
import { unreachable } from '$lib/catalog/read';
import type { PageLoad } from './$types';

/** Loads the plugins the user can connect, or null when the server cannot be reached. */
export const load: PageLoad = async ({ fetch }) => {
	// The plugins the server offers need it; without it the page still opens, to turn Work offline off.
	const connectable = await listMyPlugins(fetch).then(
		({ items }) => items,
		(error) => {
			if (unreachable(error)) return null;
			throw error;
		}
	);
	return { connectable };
};
