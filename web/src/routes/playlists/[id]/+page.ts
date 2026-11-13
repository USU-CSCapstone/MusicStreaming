import { catalog } from '$lib/catalog/client.svelte';
import { requireLibrary } from '$lib/library';
import { loadPlaylist } from '$lib/playlist.svelte';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, params }) => {
	const lib = await requireLibrary(parent);
	// Taken before reading, so the page can tell a change announced while it loads.
	const since = catalog.changes;
	// Every item at one version, as editing needs.
	return { lib, since, loaded: await loadPlaylist(fetch, lib, params.id) };
};
