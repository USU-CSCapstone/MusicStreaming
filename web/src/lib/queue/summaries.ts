// The tracks queue rows name, for the panel to show: one reach-checked lookup by IDs per
// library, at most `CHUNK` to a request (`design/queue.md` §5).

import { getTracksByIds } from '$lib/api/client';
import type { TrackSummary } from '$lib/api/types';

/** Most IDs one lookup takes. */
const CHUNK = 100;

/** The key a track is held under: its library and its ID. */
export const trackKey = (libraryId: string, trackId: string) => `${libraryId}/${trackId}`;

/**
 * The track of each of `refs`, in a map by `trackKey`. A track the library does not give, as
 * one the account cannot reach or that is gone, maps to null. A chunk whose request failed is
 * left out, so the caller asks again; this never throws and never calls the lookup with no IDs.
 */
export async function lookup(
	refs: { libraryId: string; trackId: string }[]
): Promise<Map<string, TrackSummary | null>> {
	const wanted = new Map<string, Set<string>>();
	for (const { libraryId, trackId } of refs) {
		const ids = wanted.get(libraryId) ?? new Set<string>();
		ids.add(trackId);
		wanted.set(libraryId, ids);
	}
	const out = new Map<string, TrackSummary | null>();
	await Promise.all(
		[...wanted].flatMap(([libraryId, ids]) => {
			const all = [...ids];
			return Array.from({ length: Math.ceil(all.length / CHUNK) }, async (_, i) => {
				const asked = all.slice(i * CHUNK, (i + 1) * CHUNK);
				try {
					const { items } = await getTracksByIds(fetch, libraryId, asked);
					for (const track of items) out.set(trackKey(libraryId, track.id), track);
					for (const id of asked) {
						const key = trackKey(libraryId, id);
						if (!out.has(key)) out.set(key, null);
					}
				} catch {
					// Left out, and asked for again with the next look.
				}
			});
		})
	);
	return out;
}
