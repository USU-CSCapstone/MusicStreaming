import { adminListAuditLog, listLibraries } from '$lib/api/client';
import type { PageLoad } from './$types';

// Libraries are named by their IDs in an entry; admins reach every library, so the owner's list
// names them all.
export const load: PageLoad = async ({ fetch }) => {
	const [audit, libraries] = await Promise.all([adminListAuditLog(fetch), listLibraries(fetch)]);
	return { audit, libraries: libraries.items };
};
