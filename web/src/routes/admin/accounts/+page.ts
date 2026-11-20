import { adminListUsers, listLibraries } from '$lib/api/client';
import type { PageLoad } from './$types';

// The search is in the address, so a new one runs this load again and the page starts over.
// Admins reach every library (`requirements/users.md` §5), so every one can be granted.
export const load: PageLoad = async ({ fetch, url }) => {
	const q = url.searchParams.get('q') ?? '';
	const [users, libraries] = await Promise.all([
		adminListUsers(fetch, { q }),
		listLibraries(fetch)
	]);
	return { q, users, libraries: libraries.items };
};
