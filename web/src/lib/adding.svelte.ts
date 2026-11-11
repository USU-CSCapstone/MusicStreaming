// What can be added to a playlist (`requirements/playlists.md` §4) or the queue
// (`requirements/queue.md` §1) from anywhere: songs, or a whole album, artist, or playlist. Any
// songs already in a playlist are warned about first (§3).

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
import type { Playable, Playlist, QueueRecipe, TrackSummary } from './api/types';
import type { Card } from './cards';
import { plural } from './format';
import { albumRecipe, artistRecipe, playlistRecipe } from './queue';
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
	/** Empty for a view that is no entity, such as all the songs, which only queues. */
	refs: Playable[];
	tracks: () => Promise<TrackSummary[]>;
	/**
	 * How the server makes the list, when it can: queueing it sends this, so the device need not
	 * fetch the songs, and the rows show once the server has resolved it.
	 */
	recipe?: QueueRecipe;
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
 * its `recipe` if the server can resolve one for it.
 */
export function collection(
	type: 'album' | 'artist' | 'playlist',
	id: string,
	title: string,
	count: number,
	tracks: () => Promise<TrackSummary[]>,
	recipe?: QueueRecipe
): Adding {
	return { label: `“${title}”`, count, refs: [{ type, id }], tracks, recipe };
}

/** The songs of a view that is no entity, such as the library or a search, as `recipe` resolves them: only queued. */
export function view(label: string, count: number, recipe: QueueRecipe): Adding {
	return { label, count, refs: [], tracks: async () => [], recipe };
}

/**
 * The album, artist, or playlist a card stands for. It holds no songs: the server reads them
 * when it is queued, and they are fetched only when needed here.
 */
export function cardAdding(fetch: Fetch, libraryId: string, card: Card): Adding {
	const { type, id, title, count } = card;
	if (type === 'artist') {
		const recipe = artistRecipe(id);
		return collection(type, id, title, count, artistTracks(fetch, libraryId, id), recipe);
	}
	if (type === 'album') {
		const tracks = () =>
			all((cursor) =>
				listTracks(fetch, libraryId, { albumId: id, sort: 'album', limit: 1000, cursor })
			);
		return collection(type, id, title, count, tracks, albumRecipe(id));
	}
	const tracks = async () =>
		(await all((cursor) => listPlaylistItems(fetch, libraryId, id, { cursor }))).flatMap((item) =>
			item.track ? [item.track] : []
		);
	return collection(type, id, title, count, tracks, playlistRecipe(id));
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
				const { version } = await getPlaylist(fetch, libraryId, id, true);
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
