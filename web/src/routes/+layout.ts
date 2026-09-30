import { redirect } from '@sveltejs/kit';
import { resolve } from '$app/paths';
import { getServerInfo, listLibraries } from '$lib/api/client';
import type { LayoutLoad } from './$types';

// SPA mode: static files served by the Rust server, no server rendering (`design/general.md` §1).
export const ssr = false;

export const load: LayoutLoad = async ({ fetch, route, untrack }) => {
	// A new server serves nothing but setup (`requirements/users.md` §2.1). Checked once, when
	// the app starts: `untrack` keeps navigation from loading this again.
	const { setupRequired } = await getServerInfo(fetch);
	const onSetup = untrack(() => route.id) === '/setup';
	if (setupRequired !== onSetup) redirect(307, resolve(setupRequired ? '/setup' : '/'));
	if (setupRequired) return { library: null };

	const { items } = await listLibraries(fetch);
	// One library at a time; a library switcher comes with multi-library accounts (`requirements/users.md` §5).
	return { library: items[0] ?? null };
};
