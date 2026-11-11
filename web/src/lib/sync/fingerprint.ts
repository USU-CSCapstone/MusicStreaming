// What a playlist edit's positions were made on (`mutations/playlists.ts`): the playlist's items,
// as positions see them, hashed, so an edit can tell whether the server's items are the ones it
// was made on.

import type { Playable, PlaylistItem } from '$lib/api/types';

/** How an edit names an item: a library track, or one from an external source. */
export function toPlayable(item: PlaylistItem): Playable {
	if (item.track) return { type: 'track', id: item.track.id };
	return { type: 'external', sourceId: item.external!.source!, externalId: item.external!.id };
}

/** `items` as an edit's positions see them, hashed. */
export const fingerprint = (items: readonly PlaylistItem[]) =>
	hashText(JSON.stringify(items.map(toPlayable)));

/**
 * A 53-bit hash of `text` (cyrb53), in hex: enough to tell one playlist's items from another's,
 * and quick on a playlist of thousands.
 */
export function hashText(text: string): string {
	let h1 = 0xdeadbeef;
	let h2 = 0x41c6ce57;
	for (let i = 0; i < text.length; i++) {
		const c = text.charCodeAt(i);
		h1 = Math.imul(h1 ^ c, 2654435761);
		h2 = Math.imul(h2 ^ c, 1597334677);
	}
	h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
	h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
	return (4294967296 * (2097151 & h2) + (h1 >>> 0)).toString(16);
}
