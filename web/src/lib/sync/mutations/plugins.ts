// Sharing searches with a plugin (`requirements/search.md` §6): turned on or off at once on the
// account page and sent in its turn, in the recent searches' own scope, so a plugin is told only of
// the searches made while sharing was on, whatever reaches the server when. It changes nothing in
// the device's catalog. One the server refuses is told, and the page puts its switch back
// (`routes/account/+page.svelte`).

import { setMySearchSharing } from '$lib/api/client';
import type { Definition } from '../mutation';
import { searchScope } from './searches';

/** Sharing the caller's searches with plugin `pluginId`, named `name`, turned on or off. */
export type ShareSearchesArgs = { pluginId: string; name: string; sharing: boolean };

/** Search sharing with a plugin turned on or off. */
export const shareSearches: Definition<ShareSearchesArgs> = {
	scope: searchScope,
	send: ({ pluginId, sharing }, { fetch, key }) =>
		setMySearchSharing(fetch, pluginId, sharing, key),
	what: ({ name, sharing }) =>
		sharing ? `share your searches with ${name}` : `stop sharing your searches with ${name}`
};
