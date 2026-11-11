import { getTag, listAlbums, listArtists, listTracks } from '$lib/api/client';
import { catalog } from '$lib/catalog/client.svelte';
import { requireLibrary } from '$lib/library';
import { readSort } from '$lib/sort-url';
import { FIELDS, serverSort } from '$lib/sorting';
import type { PageLoad } from './$types';

export const load: PageLoad = async ({ fetch, parent, params, url }) => {
	const lib = await requireLibrary(parent);
	// Taken before reading, so the page can tell a change announced while it loads.
	const since = catalog.changes;
	const tag = params.id;
	// The songs sort on the server; unsorted, they run album by album.
	const sort = readSort(url.searchParams, FIELDS);
	const songs = { tagAll: tag, ...(serverSort(sort) ?? { sort: 'album', order: 'asc' }) };
	const [found, albums, artists, tracks] = await Promise.all([
		getTag(fetch, lib, tag),
		listAlbums(fetch, lib, { tagAll: tag, sort: 'name' }),
		listArtists(fetch, lib, { tagAll: tag, sort: 'name' }),
		listTracks(fetch, lib, songs)
	]);
	return { lib, since, tag: found, albums, artists, songs, tracks };
};
