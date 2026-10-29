import { redirect } from '@sveltejs/kit';
import { resolve } from '$app/paths';
import { getMe, getServerInfo, listLibraries, listTagTypes } from '$lib/api/client';
import type { LayoutLoad } from './$types';

// SPA mode: static files served by the Rust server, no server rendering (`design/general.md` §1).
export const ssr = false;

export const load: LayoutLoad = async ({ fetch, route, untrack }) => {
	// A new server serves nothing but setup (`requirements/users.md` §2.1). Checked once, when
	// the app starts: `untrack` keeps navigation from loading this again.
	const { setupRequired } = await getServerInfo(fetch);
	const at = untrack(() => route.id);
	if (setupRequired !== (at === '/setup')) redirect(307, resolve(setupRequired ? '/setup' : '/'));
	if (setupRequired || at === '/login') return { me: null, library: null, tagTypes: [] };

	// Without a session, either answers `401`, which redirects to the login page.
	const [me, { items }] = await Promise.all([getMe(fetch), listLibraries(fetch)]);
	// One library at a time; a library switcher comes with multi-library accounts (`requirements/users.md` §5).
	const library = items[0] ?? null;
	// The tag types its music carries, which the sidebar lists. A library with none, or a failure
	// to read them, leaves them out rather than stopping the app (`requirements/tags.md` §8).
	const tagTypes = library ? await listTagTypes(fetch, library.id).catch(() => []) : [];
	return { me, library, tagTypes };
};
