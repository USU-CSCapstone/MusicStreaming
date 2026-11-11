import { catalog } from '$lib/catalog/client.svelte';
import { connectivity } from '$lib/connectivity.svelte';
import { requireLibrary } from '$lib/library';
import { loadPlaylist } from '$lib/playlist.svelte';
import { playlistScope } from '$lib/sync/mutations/playlists';
import { sender } from '$lib/sync/sender';
import type { PageLoad } from './$types';

/** Reads the playlist, after the server has its create where that is the only place it can be read. */
export const load: PageLoad = async ({ fetch, parent, params }) => {
	const lib = await requireLibrary(parent);
	// A playlist made here but not yet on the server is read from the device. Where the device does
	// not hold the library, the server is read, so its create is waited for first (a refused one
	// leaves the read to fail as for any playlist not there).
	if (!catalog.onDevice(lib) && !connectivity.blocked) {
		const creating = sender.unsentOf('createPlaylist', playlistScope(params.id));
		if (creating) await sender.settled(creating);
	}
	// Taken before reading, so the page can tell a change announced while it loads.
	const since = catalog.changes;
	// Every item at one version, as editing needs.
	return { lib, since, loaded: await loadPlaylist(fetch, lib, params.id) };
};
