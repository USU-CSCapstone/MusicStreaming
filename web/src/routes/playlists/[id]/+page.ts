import { all, getPlaylist, listPlaylistItems } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, params }) => {
	const lib = await requireLibrary(parent);
	const [playlist, items] = await Promise.all([
		getPlaylist(fetch, lib, params.id),
		all((cursor) => listPlaylistItems(fetch, lib, params.id, { limit: 1000, cursor }))
	]);
	// External tracks from plugins are not shown yet (`requirements/plugins.md` §10).
	return { playlist, tracks: items.flatMap((i) => (i.track ? [i.track] : [])) };
};
