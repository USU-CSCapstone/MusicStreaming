import { listLibraries } from '$lib/api/client';
import type { LayoutLoad } from './$types';

// SPA mode: static files served by the Rust server, no server rendering (`design/general.md` §1).
export const ssr = false;

export const load: LayoutLoad = async ({ fetch }) => {
	const { items } = await listLibraries(fetch);
	// One library at a time; a library switcher comes with multi-library accounts (`requirements/users.md` §5).
	return { library: items[0] ?? null };
};
