import { adminListInvites, listLibraries } from '$lib/api/client';
import type { PageLoad } from './$types';

// Admins reach every library (`requirements/users.md` §5), so the invite can offer them all.
export const load: PageLoad = async ({ fetch }) => {
	const [invites, libraries] = await Promise.all([adminListInvites(fetch), listLibraries(fetch)]);
	return { invites, libraries: libraries.items };
};
