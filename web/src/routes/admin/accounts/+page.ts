import { adminListUsers } from '$lib/api/client';
import type { PageLoad } from './$types';

// The search is in the address, so a new one runs this load again and the page starts over.
export const load: PageLoad = async ({ fetch, url }) => {
	const q = url.searchParams.get('q') ?? '';
	return { q, users: await adminListUsers(fetch, { q }) };
};
