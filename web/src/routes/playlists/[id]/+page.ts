import { requireLibrary } from '$lib/library';
import { loadPlaylist } from '$lib/playlist.svelte';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, params }) => {
	const lib = await requireLibrary(parent);
	// Every item at one version, as editing needs.
	return { loaded: await loadPlaylist(fetch, lib, params.id) };
};
