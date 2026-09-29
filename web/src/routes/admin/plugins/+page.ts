import { adminListPlugins } from '$lib/api/client';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch }) => {
	const { items } = await adminListPlugins(fetch);
	return { plugins: items };
};
