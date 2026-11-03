// Starting a queue from a view, and when Previous steps back. Moving through the queue is the
// queue's (`queue/`); tracks marked missing are skipped in place, leaving order and position
// untouched (`requirements/conventions.md` §6).

import type { QueueRecipe } from './api/types';
import { type ServerSort, type TrackSort, sortName } from './sorting';
import { isFiltered, type TagFilter } from './tag-match';

type QueueLibrary = Extract<QueueRecipe, { type: 'library' }>;

/** Previous restarts the current track once it has played this long. */
export const RESTART_AFTER_S = 3;

/**
 * What a start from a view sends in place of the tracks it shows: the recipe the server
 * resolves into the view's whole list, and how many tracks that is.
 */
export type PlayFrom = { recipe: QueueRecipe; total: number };

/** `start` itself if it can play, otherwise the first one after it that can. */
export function firstPlayable(tracks: { availability: 'available' | 'missing' }[], start: number) {
	for (let i = Math.max(start, 0); i < tracks.length; i++) {
		if (tracks[i].availability === 'available') return i;
	}
	return null;
}

/** In disc then track order, as an unsorted album plays and an artist's albums are listed. */
const BY_ALBUM: ServerSort = { sort: 'album', order: 'asc' };

/**
 * A recipe's `filters` for `filter`: the tags a view is narrowed to, as the list endpoints name
 * them. Empty for none.
 */
export function recipeFilters(filter: TagFilter): { tagAll?: string[]; tagAny?: string[] } {
	return {
		...(filter.all.length && { tagAll: filter.all }),
		...(filter.any.length && { tagAny: filter.any })
	};
}

/** An album's songs in `sort`, narrowed to `filter`'s tags, as the server resolves them. */
export const albumRecipe = (albumId: string, sort = BY_ALBUM, filter?: TagFilter): QueueRecipe => ({
	type: 'album',
	albumId,
	...(filter && isFiltered(filter) && { filters: recipeFilters(filter) }),
	...sort
});

/** The library's songs in `sort`, narrowed to `filter`'s tags, as the server resolves them. */
export const libraryRecipe = (
	sort: Pick<QueueLibrary, 'sort' | 'order'>,
	filter: TagFilter
): QueueRecipe => ({
	type: 'library',
	filters: recipeFilters(filter),
	...sort
});

/** The songs a search finds, in its ranking, narrowed to `filter`'s tags, as the server resolves them. */
export const searchRecipe = (q: string, filter: TagFilter): QueueRecipe => ({
	type: 'search',
	q,
	...(isFiltered(filter) && { filters: recipeFilters(filter) })
});

/** Every song an artist is credited on in `sort`, as the server resolves them. */
export const artistRecipe = (artistId: string, sort = BY_ALBUM): QueueRecipe => ({
	type: 'artist',
	artistId,
	scope: 'all',
	artistCredit: 'any',
	...sort
});

/**
 * A playlist's songs as the server resolves its stored playlist: in the playlist's own order, or
 * in `sort` (which has `added`, unlike the others, sent as `addedToPlaylist`), narrowed to
 * `filter`'s tags. `sort` and `order` go together or not at all.
 */
export const playlistRecipe = (
	playlistId: string,
	sort: TrackSort = null,
	filter?: TagFilter
): QueueRecipe => ({
	type: 'playlist',
	playlistId,
	...(filter && isFiltered(filter) && { filters: recipeFilters(filter) }),
	...(sort && { sort: sortName(sort.field), order: sort.order })
});

/** Whether Previous restarts the track: it is under way, or nothing has played before it. */
export function restartsOnPrevious(positionS: number, history: number): boolean {
	return positionS > RESTART_AFTER_S || history === 0;
}
