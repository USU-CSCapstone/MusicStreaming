// An image's ThumbHash as a picture, for the art that has not loaded yet
// (`requirements/offline.md` §1.1). A list shows each placeholder many times and scrolls past
// thousands, so each is decoded once, when its first row is drawn, and the most recent are kept.

import { thumbHashToDataURL } from 'thumbhash';

/** Decoded placeholders kept: a few KB each, so a screenful of lists stays cheap. */
const KEPT = 500;
const decoded = new Map<string, string | null>();

/** The picture for a base64 ThumbHash, or null where there is none or it cannot be read. */
export function placeholderUrl(hash: string | null | undefined): string | null {
	if (!hash) return null;
	let url = decoded.get(hash);
	if (url === undefined) {
		try {
			url = thumbHashToDataURL(Uint8Array.from(atob(hash), (c) => c.charCodeAt(0)));
		} catch {
			// A placeholder the server could not make well is a bad stand-in, not an error.
			url = null;
		}
		if (decoded.size >= KEPT) decoded.delete(decoded.keys().next().value!);
	} else decoded.delete(hash);
	decoded.set(hash, url);
	return url;
}
