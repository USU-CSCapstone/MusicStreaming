// Sorting a track list already on the device, in exactly the order the server's `listTracks`
// would give it: the core's one comparator (same keys, same tie-breaks, unknown values last
// either way). The core loads here, on the page, so a sort stays synchronous; only the pages
// that sort locally import this, so no other page waits for it (`design/general.md` §7.1).

import init, { sortRows as coreSortRows } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type { TrackSummary } from './api/types';
import { sortName, type TrackSort } from './sorting';

// The core loads before any list sorts; its sort is synchronous. The queue worker is told this
// same file's URL (`queue/client.svelte.ts`), so the browser fetches it once. (A no-op once
// loaded.)
await init();

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
	return Array.from(
		coreSortRows(JSON.stringify(keys), sortName(sort.field), sort.order),
		(i) => rows[i]
	);
}

/** Tracks in `sort`'s order. */
export function sortTracks(
	tracks: readonly TrackSummary[],
	sort: TrackSort
): readonly TrackSummary[] {
	return sortRows(tracks, sort, (t) => t);
}
