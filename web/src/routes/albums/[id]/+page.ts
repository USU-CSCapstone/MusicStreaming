import { all, getAlbum, listTracks } from '$lib/api/client';
import { catalog } from '$lib/catalog/client.svelte';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

/** The album and its tracks. */
export const load: PageLoad = async ({ fetch, parent, params }) => {
	const lib = await requireLibrary(parent);
	// Taken before reading, so the page can tell a change announced while it loads.
	const since = catalog.changes;
	return { lib, since, ...(await _readAlbum(fetch, lib, params.id)) };
};

/** An album and its tracks; the page reads them again with this when the library changes. */
export async function _readAlbum(fetch: typeof globalThis.fetch, lib: string, id: string) {
	const [album, tracks] = await Promise.all([
		getAlbum(fetch, lib, id),
		// Disc then track order, missing tracks in position (`requirements/albums.md` §4).
		all((cursor) => listTracks(fetch, lib, { albumId: id, sort: 'album', limit: 1000, cursor }))
	]);
	return { album, tracks };
}
