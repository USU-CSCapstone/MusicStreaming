// Sorting track lists (`requirements/conventions.md` §4.1): what each list sorts by and how the
// API names each sort. A list already on the device sorts through the core (`track-sort.ts`).

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

/**
 * Each sort as the API names it: `listTracks`' name for the fields it sorts by, and
 * `addedToPlaylist` for when a playlist took the track, which the server sorts only a playlist
 * recipe by. The core's comparator takes these names too.
 */
const NAMES = {
	title: 'name',
	artist: 'artist',
	album: 'album',
	year: 'releaseDate',
	duration: 'duration',
	added: 'addedToPlaylist'
} as const satisfies Record<SortField, string>;

/** `field` as the API names it. */
export const sortName = <F extends SortField>(field: F): (typeof NAMES)[F] => NAMES[field];

export type ServerSort = {
	sort: (typeof NAMES)[Exclude<SortField, 'added'>];
	order: SortOrder;
};

/** The `sort` and `order` to ask `listTracks` for, or `null` for the list's own order. */
export function serverSort(sort: TrackSort): ServerSort | null {
	if (!sort || sort.field === 'added') return null;
	return { sort: sortName(sort.field), order: sort.order };
}
