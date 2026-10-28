// Adding to a playlist from anywhere (`requirements/playlists.md` §4): songs, or a whole album,
// artist, or playlist. Any songs already in it are warned about first (§3).

import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import {
	ApiError,
	all,
	checkPlaylistDuplicates,
	editPlaylist,
	getPlaylist,
	listPlaylistItems,
	listTracks
} from './api/client';
import type { Playable, Playlist, QueueQuery, TrackSummary } from './api/types';
import type { Card } from './cards';
import { plural } from './format';
import type { EditablePlaylist } from './playlist.svelte';
import { toast } from './toast.svelte';

type Fetch = typeof fetch;

/**
 * What is being added. `refs` is what the server is sent, which expands an album, artist, or
 * playlist itself (`api/openapi.yaml`, `PlaylistCreate`). `tracks` gives the songs themselves,
 * in that same order, for when they are needed here: adding to the playlist open on screen,
 * which shows them at once, and leaving out those already there.
 */
export type Adding = {
	/** "“Signal”", or "3 songs". */
	label: string;
	/** How many songs, so a warning can say how many are already there. */
	count: number;
	refs: Playable[];
	tracks: () => Promise<TrackSummary[]>;
	/**
	 * The list of tracks it is, when the device does not hold them: queueing it sends this for
	 * the server to read, rather than fetching the tracks first.
	 */
	query?: QueueQuery;
};

/**
 * The playlist open on screen, if any. Adding to it goes through its store, so it shows at once
 * and its next edit is not refused as if another device had changed it.
 */
export const opened = $state<{ list: EditablePlaylist | null }>({ list: null });

export const playables = (tracks: TrackSummary[]): Playable[] =>
	tracks.map((t) => ({ type: 'track', id: t.id }));

/** Songs, by name when there is one. */
export function songs(tracks: TrackSummary[]): Adding {
	return {
		label: tracks.length === 1 ? `“${tracks[0].title}”` : plural(tracks.length, 'song'),
		count: tracks.length,
		refs: playables(tracks),
		tracks: async () => tracks
	};
}

/**
 * An album, artist, or playlist, named `title`, with `tracks` as the server expands it. Give
 * its `query` unless `tracks` are already held.
 */
export function collection(
	type: 'album' | 'artist' | 'playlist',
	id: string,
	title: string,
	count: number,
	tracks: () => Promise<TrackSummary[]>,
	query?: QueueQuery
): Adding {
	return { label: `“${title}”`, count, refs: [{ type, id }], tracks, query };
}

/** An artist's songs as `artistTracks` lists them, for the server to read. */
export const artistQuery = (artistId: string): QueueQuery => ({
	type: 'tracks',
	artistId,
	sort: 'album'
});

/**
 * The album, artist, or playlist a card stands for, which it does not hold: the tracks of an
 * album or artist are fetched when asked for or read by the server, a playlist's fetched.
 */
export function cardAdding(fetch: Fetch, libraryId: string, card: Card): Adding {
	const { type, id, title, count } = card;
	if (type === 'artist') {
		return collection(type, id, title, count, artistTracks(fetch, libraryId, id), artistQuery(id));
	}
	if (type === 'album') {
		const list = { albumId: id, sort: 'album' } as const;
		const tracks = () =>
			all((cursor) => listTracks(fetch, libraryId, { ...list, limit: 1000, cursor }));
		return collection(type, id, title, count, tracks, { type: 'tracks', ...list });
	}
	const tracks = async () =>
		(await all((cursor) => listPlaylistItems(fetch, libraryId, id, { cursor }))).flatMap((item) =>
			item.track ? [item.track] : []
		);
	return collection(type, id, title, count, tracks);
}

/** Every song by an artist, in the order the server expands an artist: album, disc, track. */
export const artistTracks = (fetch: Fetch, libraryId: string, artistId: string) => () =>
	all((cursor) => listTracks(fetch, libraryId, { artistId, sort: 'album', limit: 1000, cursor }));

/** The IDs of the songs in `what` the playlist already has, once each. */
export async function alreadyIn(fetch: Fetch, playlist: Playlist, what: Adding): Promise<string[]> {
	const list = opened.list;
	if (list?.playlist.id === playlist.id) {
		// What shows here, pending edits and all, rather than what the server has so far.
		const have = list.items.flatMap((i) => (i.track ? [i.track.id] : []));
		return (await what.tracks())
			.map((t) => t.id)
			.filter((id, k, ids) => have.includes(id) && ids.indexOf(id) === k);
	}
	const { trackIds } = await checkPlaylistDuplicates(
		fetch,
		playlist.libraryId,
		playlist.id,
		what.refs
	);
	return trackIds;
}

/**
 * Appends `what`, or only `only` of its songs, saying so at once (`requirements/general.md`
 * §3.4), and again if it fails.
 */
export async function addTo(fetch: Fetch, playlist: Playlist, what: Adding, only?: TrackSummary[]) {
	const { id, libraryId, title } = playlist;
	const list = opened.list;
	const here = list?.playlist.id === id;
	toast(`Added ${only ? songs(only).label : what.label} to “${title}”`, {
		action: here
			? undefined
			: { label: 'Open', run: () => void goto(resolve('/playlists/[id]', { id })) }
	});
	try {
		if (here) {
			// The store shows songs from their summaries, so these are the songs themselves.
			const tracks = only ?? (await what.tracks());
			list!.edit({ ops: [{ op: 'add', items: playables(tracks) }] }, tracks);
			return;
		}
		const items = only ? playables(only) : what.refs;
		// Appending names no positions, so a change made elsewhere meanwhile is no reason to
		// refuse it: it is tried again against the newer version.
		for (let attempt = 1; ; attempt++) {
			try {
				const { version } = await getPlaylist(fetch, libraryId, id);
				await editPlaylist(fetch, libraryId, id, {
					baseVersion: version,
					ops: [{ op: 'add', items }]
				});
				return;
			} catch (e) {
				if (!(e instanceof ApiError && e.code === 'version_conflict' && attempt < 3)) throw e;
			}
		}
	} catch (e) {
		toast(`Couldn’t add to “${title}”. ${e instanceof ApiError ? e.message : ''}`.trim());
	}
}
