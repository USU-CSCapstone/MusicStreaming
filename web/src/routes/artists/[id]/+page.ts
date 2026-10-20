import { getArtist, listAlbums, listTracks } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import { readSort } from '$lib/sort-url';
import { FIELDS, serverSort } from '$lib/sorting';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, params, url, untrack }) => {
	const lib = await requireLibrary(parent);
	const artistId = params.id;
	// The songs as a link or a reload asks for them. Read without depending on it: a new sort
	// fetches only the songs, on the page, rather than the artist and albums again.
	const songs = _songOrder(untrack(() => readSort(url.searchParams, FIELDS)));
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
		listTracks(fetch, lib, { artistId, ...songs })
	]);
	return { lib, artist, albums, appearances, songs, tracks };
};

/** An artist's songs in `sort`'s order; unsorted, in album order. Shared with the page. */
export function _songOrder(sort: ReturnType<typeof readSort>) {
	return serverSort(sort) ?? { sort: 'album' as const, order: 'asc' as const };
}
