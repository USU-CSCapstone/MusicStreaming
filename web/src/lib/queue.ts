// Starting a queue from a view, and when Previous steps back. Moving through the queue is the
// queue's (`queue/`); tracks marked missing are skipped in place, leaving order and position
// untouched (`requirements/conventions.md` §6).

import type { QueueRecipe } from './api/types';
import type { ServerSort } from './sorting';

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

/** An album's songs in `sort`, as the server resolves them. */
export const albumRecipe = (albumId: string, sort = BY_ALBUM): QueueRecipe => ({
	type: 'album',
	albumId,
	...sort
});

/** Every song an artist is credited on in `sort`, as the server resolves them. */
export const artistRecipe = (artistId: string, sort = BY_ALBUM): QueueRecipe => ({
	type: 'artist',
	artistId,
	scope: 'all',
	artistCredit: 'any',
	...sort
});

/** Whether Previous restarts the track: it is under way, or nothing has played before it. */
export function restartsOnPrevious(positionS: number, history: number): boolean {
	return positionS > RESTART_AFTER_S || history === 0;
}
