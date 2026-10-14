// Adding songs to a playlist from anywhere they are shown (`requirements/playlists.md` §4):
// warning first about any already in it (§3), then appending them.

import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { ApiError, checkPlaylistDuplicates, editPlaylist, getPlaylist } from './api/client';
import type { Playable, Playlist, TrackSummary } from './api/types';
import { plural } from './format';
import type { EditablePlaylist } from './playlist.svelte';
import { toast } from './toast.svelte';

type Fetch = typeof fetch;

/**
 * The playlist open on screen, if any. Adding to it goes through its store, so it shows at once
 * and its next edit is not refused as if another device had changed it.
 */
export const opened = $state<{ list: EditablePlaylist | null }>({ list: null });

export const playables = (tracks: TrackSummary[]): Playable[] =>
	tracks.map((t) => ({ type: 'track', id: t.id }));

/** "“Signal”" for one song, "3 songs" for more. */
export const describe = (tracks: TrackSummary[]) =>
	tracks.length === 1 ? `“${tracks[0].title}”` : plural(tracks.length, 'song');

/** The IDs of those of `tracks` the playlist already has, once each. */
export async function alreadyIn(
	fetch: Fetch,
	playlist: Playlist,
	tracks: TrackSummary[]
): Promise<string[]> {
	const list = opened.list;
	if (list?.playlist.id === playlist.id) {
		// What shows here, pending edits and all, rather than what the server has so far.
		const have = list.items.flatMap((i) => (i.track ? [i.track.id] : []));
		return tracks
			.map((t) => t.id)
			.filter((id, k, all) => have.includes(id) && all.indexOf(id) === k);
	}
	const { trackIds } = await checkPlaylistDuplicates(
		fetch,
		playlist.libraryId,
		playlist.id,
		playables(tracks)
	);
	return trackIds;
}

/** Appends `tracks`, saying so at once (`requirements/general.md` §3.4), and again if it fails. */
export async function addTracks(fetch: Fetch, playlist: Playlist, tracks: TrackSummary[]) {
	const { id, libraryId, title } = playlist;
	const list = opened.list;
	const here = list?.playlist.id === id;
	toast(`Added ${describe(tracks)} to “${title}”`, {
		action: here
			? undefined
			: { label: 'Open', run: () => void goto(resolve('/playlists/[id]', { id })) }
	});
	if (here) {
		list!.edit({ ops: [{ op: 'add', items: playables(tracks) }] }, tracks);
		return;
	}
	// Appending names no positions, so a change made elsewhere meanwhile is no reason to refuse
	// it: it is tried again against the newer version.
	for (let attempt = 1; ; attempt++) {
		try {
			const { version } = await getPlaylist(fetch, libraryId, id);
			await editPlaylist(fetch, libraryId, id, {
				baseVersion: version,
				ops: [{ op: 'add', items: playables(tracks) }]
			});
			return;
		} catch (e) {
			if (e instanceof ApiError && e.code === 'version_conflict' && attempt < 3) continue;
			toast(`Couldn’t add to “${title}”. ${e instanceof ApiError ? e.message : ''}`.trim());
			return;
		}
	}
}
