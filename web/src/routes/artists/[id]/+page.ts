import { getArtist, listAlbums, listTracks } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, params }) => {
	const lib = await requireLibrary(parent);
	const artistId = params.id;
	const [artist, albums, appearances, tracks] = await Promise.all([
		getArtist(fetch, lib, artistId),
		// The discography, newest first (`requirements/artists.md` §3.1).
		listAlbums(fetch, lib, { artistId, artistCredit: 'owned', sort: 'releaseDate', order: 'desc' }),
		listAlbums(fetch, lib, {
			artistId,
			artistCredit: 'featured',
			sort: 'releaseDate',
			order: 'desc'
		}),
		listTracks(fetch, lib, { artistId, sort: 'album' })
	]);
	return { lib, artist, albums, appearances, tracks };
};
