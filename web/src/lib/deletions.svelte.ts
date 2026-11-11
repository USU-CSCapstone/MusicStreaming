// Deleting a playlist can be undone (`requirements/playlists.md` §4). It is a mutation like any
// write (`sync/mutations/playlists.ts`): the device deletes it at once, so it leaves every list,
// but the log holds it back until its Undo toast has closed, since the API has no way to bring a
// deleted playlist back. Undo cancels it, which takes the device's deletion back; a closed tab
// does not lose it, whichever tab finds it due sends it.

import { SvelteSet } from 'svelte/reactivity';
import type { Playlist } from './api/types';
import { marks } from './marks.svelte';
import { sender } from './sync/sender';
import { TOAST_MS, toast } from './toast.svelte';

/** How long a deletion is held at a time: its toast and a little over. Held again while it is open. */
export const HOLD_MS = TOAST_MS + 1000;

/**
 * Playlists deleted here, which lists leave out at once even where the device does not hold the
 * library to delete them from, until the server has answered or Undo.
 */
export const deleted = new SvelteSet<string>();

/**
 * Deletes `playlist` and offers Undo. Once the toast goes any other way — timed out, dismissed, or
 * replaced — the deletion is no longer undone, and is sent when its time comes, after the edits
 * made to the playlist before it.
 */
export function deleteWithUndo(playlist: Playlist) {
	const { id, libraryId, title } = playlist;
	const queued = sender.enqueue(
		'deletePlaylist',
		{ libraryId, playlistId: id, title },
		{ notBefore: Date.now() + HOLD_MS }
	);
	if (!queued) {
		toast(`Couldn’t delete “${title}”.`);
		return;
	}
	deleted.add(id);
	// While its toast is open, even held by the pointer or focus, the deletion is held back a little
	// further each time; a closed tab leaves it to go once the last of these has passed.
	const keep = setInterval(
		() => sender.hold(queued.key, Date.now() + HOLD_MS),
		(HOLD_MS - TOAST_MS) * 2
	);
	// Refused, or taken back: it is there again.
	void sender.settled(queued.key).then((outcome) => {
		clearInterval(keep);
		if (outcome === 'dropped' || outcome === 'cancelled') deleted.delete(id);
	});

	/** Undo: stops holding the deletion and cancels it, saying so if it had been sent already. */
	const undo = () => {
		clearInterval(keep);
		void sender.cancel(queued.key).then((cancelled) => {
			if (!cancelled) toast(`“${title}” was deleted already.`);
		});
	};
	/** The toast closed without Undo, so the deletion can no longer be undone and goes now. */
	const decided = () => {
		clearInterval(keep);
		sender.hold(queued.key, Date.now());
		// Deleting Liked Songs unliked everything in it (`requirements/playlists.md` §11).
		if (playlist.kind === 'liked') marks.unlikeAll(libraryId);
	};
	toast(`Deleted “${title}”`, { action: { label: 'Undo', run: undo }, onclose: decided });
}
