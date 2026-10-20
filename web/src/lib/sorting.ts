// Sorting track lists (`requirements/conventions.md` §4.1). A list already on the device sorts
// here, in exactly the order the server's `listTracks` would give it: the same keys, the same
// tie-breaks, unknown values last either way.

import type { TrackSummary } from './api/types';

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
 * Compares by Unicode code point, as the server compares the UTF-8 bytes of a key. Plain `<`
 * compares UTF-16 code units, which puts characters past U+FFFF before U+E000–U+FFFF; and
 * `localeCompare` would make the order depend on the device.
 */
export function compareKeys(a: string, b: string): number {
	const n = Math.min(a.length, b.length);
	for (let i = 0; i < n; i++) {
		const x = a.charCodeAt(i);
		const y = b.charCodeAt(i);
		if (x !== y) return codePointRank(x) - codePointRank(y);
	}
	return a.length - b.length;
}

/** Surrogates (U+D800–U+DFFF) stand for code points above every other code unit. */
function codePointRank(unit: number): number {
	if (unit < 0xd800) return unit;
	return unit < 0xe000 ? unit + 0x2000 : unit - 0x800;
}

/** IDs are canonical decimal integers, so a shorter one is smaller. */
function compareIds(a: string, b: string): number {
	return a.length - b.length || (a < b ? -1 : a > b ? 1 : 0);
}

const compareNumbers = (a: number, b: number) => a - b;
/** The scanner files a track with no disc as disc 0; the API sends it as `null`. */
const disc = (t: TrackSummary) => t.discNumber ?? 0;
/** A track with no number sorts last on its disc. */
const number = (t: TrackSummary) => t.trackNumber ?? Number.MAX_SAFE_INTEGER;

/** Album order, the server's `album` sort: album, then disc, then number. */
function compareAlbumOrder(a: TrackSummary, b: TrackSummary): number {
	return (
		compareKeys(a.sortKeys.album, b.sortKeys.album) ||
		compareIds(a.album.id, b.album.id) ||
		compareNumbers(disc(a), disc(b)) ||
		compareNumbers(number(a), number(b)) ||
		compareIds(a.id, b.id)
	);
}

/** A field's leading value, and whether a row has none, which sorts it last. */
type Lead<T> = { unknown: (row: T) => boolean; compare: (a: T, b: T) => number };

/**
 * Each field's order, matching `TrackSort::sort` on the server: a leading value that may be
 * unknown, then what breaks its ties. `added` reads the row's date added to the playlist.
 */
function order<T>(
	field: SortField,
	track: (row: T) => TrackSummary,
	added: (row: T) => string
): { lead: Lead<T>; rest: (a: T, b: T) => number } {
	const by =
		(f: (a: TrackSummary, b: TrackSummary) => number) =>
		(a: T, b: T): number =>
			f(track(a), track(b));
	switch (field) {
		case 'title':
			return {
				lead: {
					unknown: (r) => track(r).sortKeys.title === '',
					compare: by((a, b) => compareKeys(a.sortKeys.title, b.sortKeys.title))
				},
				rest: by((a, b) => compareIds(a.id, b.id))
			};
		case 'artist':
			return {
				lead: {
					unknown: (r) => track(r).sortKeys.artist === '',
					compare: by((a, b) => compareKeys(a.sortKeys.artist, b.sortKeys.artist))
				},
				rest: by(
					(a, b) => compareKeys(a.sortKeys.title, b.sortKeys.title) || compareIds(a.id, b.id)
				)
			};
		case 'album':
			return {
				lead: {
					unknown: (r) => track(r).sortKeys.album === '',
					compare: by((a, b) => compareKeys(a.sortKeys.album, b.sortKeys.album))
				},
				rest: by(compareAlbumOrder)
			};
		case 'year':
			// The full release date, though only the year shows.
			return {
				lead: {
					unknown: (r) => track(r).releaseDate == null,
					compare: by((a, b) => compareKeys(a.releaseDate ?? '', b.releaseDate ?? ''))
				},
				rest: by(compareAlbumOrder)
			};
		case 'duration':
			return {
				lead: { unknown: () => false, compare: by((a, b) => a.durationUs - b.durationUs) },
				rest: by((a, b) => compareIds(a.id, b.id))
			};
		case 'added':
			// ISO timestamps in UTC order as text. Ties keep the playlist's order.
			return {
				lead: { unknown: () => false, compare: (a, b) => compareKeys(added(a), added(b)) },
				rest: () => 0
			};
	}
}

/**
 * `rows` in `sort`'s order, as a new array; unsorted, `rows` as they are. Rows with no value
 * sort last in either direction, and rows the sort cannot tell apart, such as a track a
 * playlist holds twice, keep the order they had.
 */
export function sortRows<T>(
	rows: readonly T[],
	sort: TrackSort,
	track: (row: T) => TrackSummary,
	added: (row: T) => string = () => ''
): readonly T[] {
	if (!sort) return rows;
	const { lead, rest } = order(sort.field, track, added);
	const sign = sort.order === 'asc' ? 1 : -1;
	return rows.toSorted((a, b) => {
		const ua = lead.unknown(a);
		const ub = lead.unknown(b);
		if (ua !== ub) return ua ? 1 : -1;
		// Among unknowns the lead is the same for all, so the rest decides.
		return sign * ((ua ? 0 : lead.compare(a, b)) || rest(a, b));
	});
}

/** Tracks in `sort`'s order. */
export function sortTracks(
	tracks: readonly TrackSummary[],
	sort: TrackSort
): readonly TrackSummary[] {
	return sortRows(tracks, sort, (t) => t);
}
