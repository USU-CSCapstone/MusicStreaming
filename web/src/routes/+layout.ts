import { redirect } from '@sveltejs/kit';
import { resolve } from '$app/paths';
import { getMe, getServerInfo, listLibraries, listTagTypes } from '$lib/api/client';
import type { User } from '$lib/api/types';
import { catalog } from '$lib/catalog/client.svelte';
import { claim, logoutPending, owner, remember } from '$lib/last-known';
import { finishLogout } from '$lib/logout';
import type { LayoutLoad } from './$types';

// SPA mode: static files served by the Rust server, no server rendering (`design/general.md` §1).
export const ssr = false;

/**
 * Makes the device `me`'s. Whatever another account left on it is wiped first, before anything
 * of `me`'s is kept or connected, so no one is ever given a file of someone else's.
 */
async function adopt(me: User) {
	if (owner() === me.id) return;
	if (owner()) {
		await catalog.signOut();
		remember({ me });
	}
	claim(me.id);
}

export const load: LayoutLoad = async ({ fetch, route, untrack }) => {
	// A logout the server has not heard of: its session may still be good, so only the login page
	// shows, with no way back in, and the server is told as soon as it can be. The wipe is done
	// again, in case the page closed before it finished.
	if (logoutPending()) {
		void catalog.signOut();
		void finishLogout(fetch);
		if (untrack(() => route.id) !== '/login') redirect(307, resolve('/login'));
		return { me: null, library: null, tagTypes: [] };
	}
	// A new server serves nothing but setup (`requirements/users.md` §2.1). Checked once, when
	// the app starts: `untrack` keeps navigation from loading this again.
	const { setupRequired } = await getServerInfo(fetch);
	const at = untrack(() => route.id);
	if (setupRequired !== (at === '/setup')) redirect(307, resolve(setupRequired ? '/setup' : '/'));
	// The login page is for someone with no session: where the session is still good, going on
	// would otherwise delete the device's catalog under it.
	if (
		at === '/login' &&
		(await getMe(fetch).then(
			() => true,
			() => false
		))
	)
		redirect(307, resolve('/'));
	if (setupRequired || at === '/login') return { me: null, library: null, tagTypes: [] };

	// Without a session, either answers `401`, which redirects to the login page.
	// The libraries are asked for with the user. Only where the account changed are they asked
	// again, as what was kept, or a first answer that raced the wipe, may be the last account's.
	const asked = listLibraries(fetch);
	asked.catch(() => {});
	const me = await getMe(fetch);
	const changed = owner() !== me.id;
	await adopt(me);
	const { items } = await (changed ? listLibraries(fetch) : asked);
	// One library at a time; a library switcher comes with multi-library accounts (`requirements/users.md` §5).
	const library = items[0] ?? null;
	// The tag types its music carries, which the sidebar lists. A library with none, or a failure
	// to read them, leaves them out rather than stopping the app (`requirements/tags.md` §8).
	const tagTypes = library ? await listTagTypes(fetch, library.id).catch(() => []) : [];
	return { me, library, tagTypes };
};
