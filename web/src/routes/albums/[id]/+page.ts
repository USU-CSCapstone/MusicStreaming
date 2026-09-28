import { all, getAlbum, listTracks } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, params }) => {
	const lib = await requireLibrary(parent);
	const [album, tracks] = await Promise.all([
		getAlbum(fetch, lib, params.id),
		// Disc then track order, missing tracks in position (`requirements/albums.md` §4).
		all((cursor) =>
			listTracks(fetch, lib, { albumId: params.id, sort: 'album', limit: 1000, cursor })
		)
	]);
	return { album, tracks };
};
