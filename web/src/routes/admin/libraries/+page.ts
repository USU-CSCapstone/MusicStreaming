import { adminListScans, listLibraries } from '$lib/api/client';
import type { Scan } from '$lib/api/types';
import type { PageLoad } from './$types';

// Admins reach every library (`requirements/users.md` §5), so this lists them all.
export const load: PageLoad = async ({ fetch }) => {
	const { items } = await listLibraries(fetch);
	const scans = await Promise.all(items.map((l) => adminListScans(fetch, l.id)));
	return {
		libraries: items,
		/** Each library's scans, by its ID: what is under way, and the last to finish. */
		scans: Object.fromEntries(items.map((l, i) => [l.id, scans[i].items])) as Record<string, Scan[]>
	};
};
