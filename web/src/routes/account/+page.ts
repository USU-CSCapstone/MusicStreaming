import { listMyPlugins } from '$lib/api/client';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
	const { items } = await listMyPlugins(fetch);
	return { connectable: items };
};
