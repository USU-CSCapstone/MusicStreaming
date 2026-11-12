// What a commit to a library's copy on the device changed, in the kinds a list can depend on, so a
// page reloads only for changes to what it shows (`live.svelte.ts`).

/**
 * The kinds of rows a commit changes. A like is a change to `liked` and to `playlists`: Liked
 * Songs is a playlist. `searches` are the account's recent searches.
 */
export const KINDS = [
	'tracks',
	'albums',
	'artists',
	'tags',
	'playlists',
	'liked',
	'hidden',
	'searches'
] as const;

/** One kind of rows a commit changes. */
export type Kind = (typeof KINDS)[number];

/** Every kind: what is told when it is not known what changed. */
export const ALL: readonly Kind[] = KINDS;

/** One entry of a feed, as far as its kind goes. */
type Entry = {
	op: string;
	kind: string;
	id: string;
	rows?: Record<string, Record<string, unknown>[]>;
};

/**
 * Adds to `found` the kinds the feed entries `changes` changed. `liked` is the ID of the user's
 * Liked Songs in the library, which a playlist entry names like any other; one that makes it
 * counts as a like. An entry for the library itself adds nothing, and one of a kind not known adds
 * every kind.
 */
export function collect(found: Set<Kind>, changes: readonly Entry[], liked?: string) {
	for (const { kind, id, rows } of changes) {
		if (kind === 'track') found.add('tracks');
		else if (kind === 'album') found.add('albums');
		else if (kind === 'artist') found.add('artists');
		else if (kind === 'tag') found.add('tags');
		else if (kind === 'hiddenTrack') found.add('hidden');
		else if (kind === 'recentSearch') found.add('searches');
		else if (kind === 'playlist' || kind === 'playlistItem') {
			found.add('playlists');
			if (id === liked || rows?.playlists?.[0]?.kind === 'liked') found.add('liked');
		} else if (kind !== 'library') for (const every of ALL) found.add(every);
	}
}

/**
 * What `collect` found, as the kinds to tell: every kind if it found none, as when only a library's
 * own counts changed.
 */
export const told = (found: ReadonlySet<Kind>): Kind[] => (found.size > 0 ? [...found] : [...ALL]);
