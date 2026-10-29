import { getTag, listAlbums, listArtists, listTracks } from '$lib/api/client';
import { requireLibrary } from '$lib/library';
import { readSort } from '$lib/sort-url';
import { FIELDS, serverSort } from '$lib/sorting';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, params, url }) => {
	const lib = await requireLibrary(parent);
	const tag = params.id;
	// The songs sort on the server; unsorted, they run album by album.
	const sort = readSort(url.searchParams, FIELDS);
	const songs = { tag, ...(serverSort(sort) ?? { sort: 'album', order: 'asc' }) };
	const [found, albums, artists, tracks] = await Promise.all([
		getTag(fetch, lib, tag),
		listAlbums(fetch, lib, { tag, sort: 'name' }),
		listArtists(fetch, lib, { tag, sort: 'name' }),
		listTracks(fetch, lib, songs)
	]);
	return { lib, tag: found, albums, artists, songs, tracks };
};
