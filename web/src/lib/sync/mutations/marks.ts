// Likes and hides (`requirements/playlists.md` §11, `requirements/conventions.md` §8): one mutation
// says what a track is now, liked or not and hidden or not, and leaves out what it does not change.
// Each track is a scope of its own, and a change made while one is unsent folds into it, so only
// the track's latest state is sent, as one request where it can be: a like unhides and a hide
// unlikes. The device makes the same change at once through the server's own code.

import { hideTrack, likeTrack, unhideTrack, unlikeTrack } from '$lib/api/client';
import type { Definition, Writes } from '../mutation';

/** What a like or hide says of a track; what it leaves out it does not change. */
export type MarkState = { liked?: boolean; hidden?: boolean };

/** A like, unlike, hide or unhide of `track` in `library`. */
export type MarkArgs = MarkState & {
	library: string;
	track: string;
	/** What finishes "Couldn’t …", such as `like “Signal”`. */
	what: string;
	/** The ID a like gives Liked Songs if it has to make it again, on the device and the server. */
	likedId?: string;
};

/** A fresh ID for a playlist the client makes, such as Liked Songs again: a positive 63-bit integer in decimal, as the server's are. */
export function mintId(): string {
	const [n] = crypto.getRandomValues(new BigUint64Array(1));
	return String(n >> 1n || 1n);
}

/** The catalog writes a mark makes of its track, in the order the server makes its requests. */
function writes({ liked, hidden, track, likedId }: MarkArgs): Writes['calls'] {
	if (liked) return [{ name: 'like', args: { trackId: track, likedId } }];
	if (hidden) return [{ name: 'hide', args: { trackId: track } }];
	return [
		...(liked === false ? [{ name: 'unlike', args: { trackId: track } }] : []),
		...(hidden === false ? [{ name: 'unhide', args: { trackId: track } }] : [])
	];
}

/** A track's latest like and hide. */
export const mark: Definition<MarkArgs> = {
	scope: ({ library, track }) => `mark:${library}/${track}`,
	async send(args, { fetch, key }) {
		const { library, track, liked, hidden, likedId } = args;
		if (liked) await likeTrack(fetch, library, track, likedId, key);
		else if (hidden) await hideTrack(fetch, library, track, key);
		else {
			// Two requests, so two keys.
			if (liked === false) await unlikeTrack(fetch, library, track, `${key}-unlike`);
			if (hidden === false) await unhideTrack(fetch, library, track, `${key}-unhide`);
		}
	},
	apply: (args) => ({ library: args.library, calls: writes(args) }),
	merge: (pending, next) => ({
		...pending,
		what: next.what,
		liked: next.liked ?? pending.liked,
		hidden: next.hidden ?? pending.hidden,
		likedId: pending.likedId ?? next.likedId
	}),
	what: (args) => args.what
};
