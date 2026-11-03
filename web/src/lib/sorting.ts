// Sorting track lists (`requirements/conventions.md` §4.1). A list already on the device sorts
// here, in exactly the order the server's `listTracks` would give it: the core's one comparator
// (same keys, same tie-breaks, unknown values last either way).

import init, { sortRows as coreSortRows } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type { TrackSummary } from './api/types';

// The core loads before any list sorts; its sort is synchronous. The queue worker is told this
// same file's URL (`queue/client.svelte.ts`), so the browser fetches it once. (A no-op once
// loaded.)
await init();

export type SortField = 'title' | 'artist' | 'album' | 'year' | 'duration' | 'added';
export type SortOrder = 'asc' | 'desc';
/** `null` is unsorted: the list's own order. */
export type TrackSort = { field: SortField; order: SortOrder } | null;

// What each kind of list sorts by, in the order its columns show.

/** Every track list. */
export const FIELDS = [
	'title',
	'artist',
	'album',
	'year',
	'duration'
] as const satisfies SortField[];
/** Playlists add when each track was added to them (`requirements/playlists.md` §10). */
export const PLAYLIST_FIELDS = [
	'title',
	'artist',
	'album',
	'year',
	'added',
	'duration'
] as const satisfies SortField[];
/** An album's own tracks share an album and, nearly always, a year. */
export const ALBUM_FIELDS = ['title', 'artist', 'duration'] as const satisfies SortField[];

/** One click on a column: ascending, then descending, then unsorted. Another column starts over. */
export function nextSort(sort: TrackSort, field: SortField): TrackSort {
	if (sort?.field !== field) return { field, order: 'asc' };
	return sort.order === 'asc' ? { field, order: 'desc' } : null;
}

/** `listTracks`' name for each sort. Date added to a playlist is never sorted by the server. */
const SERVER = {
	title: 'name',
	artist: 'artist',
	album: 'album',
	year: 'releaseDate',
	duration: 'duration'
} as const;

export type ServerSort = { sort: (typeof SERVER)[keyof typeof SERVER]; order: SortOrder };

/** The `sort` and `order` to ask `listTracks` for, or `null` for the list's own order. */
export function serverSort(sort: TrackSort): ServerSort | null {
	if (!sort || sort.field === 'added') return null;
	return { sort: SERVER[sort.field], order: sort.order };
}

/**
 * `rows` in `sort`'s order, as a new array; unsorted, `rows` as they are. The order is the
 * core's (`sort_rows`), the one the server reproduces: rows with no value sort last in either
 * direction, and rows the sort cannot tell apart, such as a track a playlist holds twice, keep
 * the order they had. `added` reads the row's date added to the playlist.
 */
export function sortRows<T>(
	rows: readonly T[],
	sort: TrackSort,
	track: (row: T) => TrackSummary,
	added: (row: T) => string = () => ''
): readonly T[] {
	if (!sort) return rows;
	const keys = rows.map((row) => {
		const t = track(row);
		return {
			id: t.id,
			titleKey: t.sortKeys.title,
			artistKey: t.sortKeys.artist,
			albumKey: t.sortKeys.album,
			albumId: t.album.id,
			releaseDate: t.releaseDate,
			disc: t.discNumber,
			number: t.trackNumber,
			durationUs: t.durationUs,
			added: added(row)
		};
	});
	return Array.from(coreSortRows(JSON.stringify(keys), sort.field, sort.order), (i) => rows[i]);
}

/** Tracks in `sort`'s order. */
export function sortTracks(
	tracks: readonly TrackSummary[],
	sort: TrackSort
): readonly TrackSummary[] {
	return sortRows(tracks, sort, (t) => t);
}
