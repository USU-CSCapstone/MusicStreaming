// Deleting a playlist can be undone (`requirements/playlists.md` §4). It disappears at once,
// and the DELETE goes only once its Undo toast is gone: the API has no way to bring a deleted
// playlist back, so waiting is the undo.

import { SvelteSet } from 'svelte/reactivity';
import { ApiError, deletePlaylist } from './api/client';
import type { Playlist } from './api/types';
import { toast, toaster } from './toast.svelte';

type Fetch = typeof fetch;

/** Playlists deleted here, sent or still undoable, which lists leave out. */
export const deleted = new SvelteSet<string>();

/**
 * Sends each deletion still waiting on its toast; `keepalive` for one outliving the page.
 * Nothing shows this, so nothing needs to react to it.
 */
// eslint-disable-next-line svelte/prefer-svelte-reactivity
const waiting = new Map<string, (keepalive?: boolean) => Promise<void>>();

/**
 * Hides `playlist` and offers Undo. Once the toast goes any other way — timed out, dismissed,
 * or replaced — the deletion is sent, after `settle`, which lets pending edits finish first.
 */
export function deleteWithUndo(fetch: Fetch, playlist: Playlist, settle?: () => Promise<void>) {
	const { id, libraryId, title } = playlist;
	let decided = false;

	async function send(keepalive = false) {
		if (decided) return;
		decided = true;
		waiting.delete(id);
		try {
			// A page being closed has no time to wait.
			if (!keepalive) await settle?.();
			const f: Fetch = keepalive ? (input, init) => fetch(input, { ...init, keepalive }) : fetch;
			await deletePlaylist(f, libraryId, id);
		} catch (e) {
			// Already gone is what was wanted.
			if (e instanceof ApiError && e.status === 404) return;
			deleted.delete(id);
			toast(`Couldn’t delete “${title}”. ${e instanceof ApiError ? e.message : ''}`.trim());
		}
	}

	function undo() {
		if (decided) return;
		decided = true;
		waiting.delete(id);
		deleted.delete(id);
	}

	deleted.add(id);
	waiting.set(id, send);
	toast(`Deleted “${title}”`, { action: { label: 'Undo', run: undo }, onclose: () => void send() });
}

// Closing the tab is not Undo: what is waiting goes now, and its toast with it.
if (typeof window !== 'undefined') {
	addEventListener('pagehide', () => {
		for (const send of waiting.values()) void send(true);
		toaster.dismiss();
	});
}
