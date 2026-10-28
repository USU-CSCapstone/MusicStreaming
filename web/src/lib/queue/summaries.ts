// The tracks the queue panel's rows show: an item names a track through the queue, and the
// track's summary comes from the reach-checked batch lookup, only for the rows in view
// (`design/queue.md` §5).

import { getTracks } from '$lib/api/client';
import type { TrackSummary } from '$lib/api/types';
import { queue } from './client.svelte';

/** Most IDs one lookup takes. */
const CHUNK = 100;
/** How many summaries are kept before the cache starts over, so scrolling a long queue stays bounded. */
const KEEP = 2000;

/** Summaries by track ID; null for a track the lookup left out, which the account cannot reach or is gone. */
const known = new Map<string, TrackSummary | null>();

/**
 * The summary each of `items` shows, in a map by item. An item the queue does not have, or whose
 * track is out of reach, maps to null. An item whose lookup failed, such as offline, is left out
 * so that it is asked for again.
 */
export async function lookup(items: string[]): Promise<Map<string, TrackSummary | null>> {
	if (known.size > KEEP) known.clear();
	const refs = await Promise.all(items.map((item) => queue.track(item).catch(() => undefined)));
	const wanted: Record<string, Set<string>> = {};
	for (const ref of refs) {
		if (ref && !known.has(ref.trackId)) (wanted[ref.libraryId] ??= new Set()).add(ref.trackId);
	}
	await Promise.all(
		Object.entries(wanted).flatMap(([library, ids]) => {
			const all = [...ids];
			return Array.from({ length: Math.ceil(all.length / CHUNK) }, async (_, i) => {
				const asked = all.slice(i * CHUNK, (i + 1) * CHUNK);
				try {
					const found = (await getTracks(fetch, library, asked)).items;
					for (const track of found) known.set(track.id, track);
					for (const id of asked) if (!known.has(id)) known.set(id, null);
				} catch {
					// Left unknown, and asked for again with the next look.
				}
			});
		})
	);
	const out = new Map<string, TrackSummary | null>();
	items.forEach((item, i) => {
		const ref = refs[i];
		if (ref === undefined) return;
		if (ref === null) out.set(item, null);
		else if (known.has(ref.trackId)) out.set(item, known.get(ref.trackId) ?? null);
	});
	return out;
}
