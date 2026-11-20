// Keeping a search to the album, artist, playlist or tag it was started from
// (`requirements/search.md` §5). The choice is made from the results: a search started from a
// container's page offers to keep to it, and searching everywhere stays one step away. Both are
// in the URL, so Back, a reload and a link keep them: `from` the container offered, and `within`
// the one the search keeps to.

import { getAlbum, getArtist, getPlaylist, getTag } from './api/client';
import { albumTitle, artistName } from './format';
import type { SearchTab } from './search-tabs';

/** The kinds of container, as `within` names them. */
export type ContainerKind = 'album' | 'artist' | 'playlist' | 'tag';

/** A container a search can keep to, as `album:ID`, and its name. */
export type Container = { within: string; kind: ContainerKind; name: string };

/** The query parameters: the container offered, and whether the search keeps to it. */
export const FROM_PARAM = 'from';
export const WITHIN_PARAM = 'within';

/** The pages of containers, by the kind each is. */
const PAGES: Record<string, ContainerKind> = {
	'/albums/[id]': 'album',
	'/artists/[id]': 'artist',
	'/playlists/[id]': 'playlist',
	'/tags/[id]': 'tag'
};

/** The container of the page at `routeId` with `params`, as `within` names it, if it is one. */
export function containerOf(routeId: string | null, params: Record<string, string>): string | null {
	const kind = routeId ? PAGES[routeId] : undefined;
	return kind && params.id ? `${kind}:${params.id}` : null;
}

/**
 * The tabs of what a container holds (`api/openapi.yaml`, `Within`): an album and a playlist
 * their songs, an artist their albums and songs, and a tag the albums, artists and songs that
 * carry it, and each its songs' lyrics. A tab of anything else would always be empty.
 */
export const HELD: Record<ContainerKind, SearchTab[]> = {
	album: ['tracks', 'lyrics'],
	artist: ['albums', 'tracks', 'lyrics'],
	playlist: ['tracks', 'lyrics'],
	tag: ['albums', 'artists', 'tracks', 'lyrics']
};

/** What kind of container `within` names, if it names one. */
export function kindOf(within: string): ContainerKind | null {
	const kind = within.split(':')[0];
	return kind in HELD ? (kind as ContainerKind) : null;
}

/**
 * The container `within` names, with its name as its own page shows it, or null if it names none
 * the caller reaches.
 */
export async function readContainer(
	fetch: typeof globalThis.fetch,
	libraryId: string,
	within: string
): Promise<Container | null> {
	const kind = kindOf(within);
	const id = within.slice(within.indexOf(':') + 1);
	if (!kind || !id) return null;
	try {
		const name =
			kind === 'album'
				? albumTitle((await getAlbum(fetch, libraryId, id)).title)
				: kind === 'artist'
					? artistName((await getArtist(fetch, libraryId, id)).name)
					: kind === 'playlist'
						? (await getPlaylist(fetch, libraryId, id)).title
						: (await getTag(fetch, libraryId, id)).name;
		return { within, kind, name };
	} catch {
		return null;
	}
}

/** The query parameters of a search offered `from` and keeping to it if `within`. */
export function scopeParams(from: string | null, within: boolean): Record<string, string> {
	if (!from) return {};
	return { [FROM_PARAM]: from, ...(within && { [WITHIN_PARAM]: from }) };
}
