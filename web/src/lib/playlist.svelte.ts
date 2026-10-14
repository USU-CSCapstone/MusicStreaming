// One playlist as the user edits it. Every edit shows at once and the server confirms it
// afterwards (`requirements/general.md` §3.4, `requirements/playlists.md` §4–§5).
//
// Edits go to the server one at a time, each against the version the one before produced, so
// a device's own edits never conflict. A conflict means another device changed the playlist:
// everything pending here is dropped, the playlist reloads, and the user is told. Any other
// refusal rolls back everything pending, since later edits were made on top of the refused one.
//
// Pending edits last as long as the page. The durable command queue that survives a closed tab
// comes with the worker (`design/general.md` §4).

import { ApiError, editPlaylist, getPlaylist, listPlaylistItems } from './api/client';
import type { Playable, Playlist, PlaylistEdit, PlaylistItem, TrackSummary } from './api/types';
import { applyOps } from './playlist-ops';
import { toast } from './toast.svelte';

type Fetch = typeof fetch;

/** An edit without its base version, which is filled in when it is sent. */
export type PlaylistChange = Omit<PlaylistEdit, 'baseVersion'>;

/** A playlist and every item in it, at one version. */
export type LoadedPlaylist = { playlist: Playlist; items: PlaylistItem[] };

type Pending = {
	change: PlaylistChange;
	/** The `Idempotency-Key`, the same on every attempt. */
	key: string;
	/** The edit's ops, applied as they were here, with the same added tracks. */
	apply: (items: PlaylistItem[]) => PlaylistItem[];
};

export const CONFLICT_MESSAGE =
	'This playlist changed on another device, so your change wasn’t applied.';

/** The longest wait between attempts to reach the server. */
const MAX_RETRY_MS = 30_000;

export class EditablePlaylist {
	/** As shown: what the server confirmed, with pending edits applied. */
	playlist = $state<Playlist>()!;
	/** Every item, in order; an item's index is its position for edits. */
	items = $state<PlaylistItem[]>([]);

	#fetch: Fetch;
	#confirmed: LoadedPlaylist;
	#queue: Pending[] = [];
	#sending: Promise<void> | null = null;

	constructor(fetch: Fetch, loaded: LoadedPlaylist) {
		this.#fetch = fetch;
		this.#confirmed = loaded;
		this.playlist = loaded.playlist;
		this.items = loaded.items;
	}

	/** Whether there are edits the server has not confirmed yet. */
	get saving(): boolean {
		return this.#queue.length > 0;
	}

	/**
	 * Applies `change` here at once, and queues it for the server. Adds name tracks, whose
	 * summaries `tracks` gives, so what shows here is exactly what the server stores: expand an
	 * album or artist before adding it. A position out of range throws, and nothing applies.
	 */
	edit(change: PlaylistChange, tracks: TrackSummary[] = []) {
		const { ops } = change;
		// Only the timestamp is kept, so nothing needs to react to this date.
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		const addedAt = new Date().toISOString();
		const resolve = (added: Playable[]) =>
			added.map((p) => {
				const track = p.type === 'track' ? tracks.find((t) => t.id === p.id) : undefined;
				if (!track) throw new Error('Adding a track needs its summary.');
				return { track, addedAt };
			});
		const apply = (items: PlaylistItem[]) => (ops ? applyOps(items, ops, resolve) : items);

		this.items = apply(this.items);
		this.#queue.push({ change, key: crypto.randomUUID(), apply });
		this.#present();
		this.#sending ??= this.#send();
	}

	/** Settles once every pending edit is confirmed, refused, or dropped. */
	async idle(): Promise<void> {
		while (this.#sending) await this.#sending;
	}

	async #send() {
		let attempt = 0;
		while (this.#queue.length > 0) {
			const next = this.#queue[0];
			const { playlist, items } = this.#confirmed;
			try {
				const confirmed = await editPlaylist(
					this.#fetch,
					playlist.libraryId,
					playlist.id,
					{ ...next.change, baseVersion: playlist.version },
					next.key
				);
				this.#confirmed = { playlist: confirmed, items: next.apply(items) };
				this.#queue.shift();
				attempt = 0;
				this.#present();
			} catch (e) {
				if (unreachable(e)) {
					// The same key on every attempt, so an edit that did apply is not applied twice.
					await new Promise((r) => setTimeout(r, retryDelay(e, attempt++)));
				} else if (e instanceof ApiError && e.code === 'version_conflict') {
					await this.#reload();
				} else {
					this.#reset();
					toast(`Couldn’t save your change. ${e instanceof ApiError ? e.message : ''}`.trim());
				}
			}
		}
		this.#sending = null;
	}

	/** After a conflict: the server's playlist, with nothing of this device's pending. */
	async #reload() {
		const { playlist } = this.#confirmed;
		try {
			this.#confirmed = await loadPlaylist(this.#fetch, playlist.libraryId, playlist.id);
		} catch {
			// Shown as last confirmed; the next edit meets the conflict again and reloads.
		}
		// Edits made while reloading were made on the old playlist too.
		this.#reset();
		toast(CONFLICT_MESSAGE);
	}

	/** Drops every pending edit, showing what the server last confirmed. */
	#reset() {
		this.#queue = [];
		this.items = this.#confirmed.items;
		this.playlist = this.#confirmed.playlist;
	}

	/**
	 * The confirmed details with pending ones over them. Totals count what shows here, which
	 * holds every item; the artwork waits for the server, except that an empty playlist has none.
	 */
	#present() {
		const playlist = { ...this.#confirmed.playlist };
		if (this.#queue.length === 0) {
			this.playlist = playlist;
			return;
		}
		for (const { change } of this.#queue) {
			if (change.title !== undefined) playlist.title = change.title;
			if (change.description !== undefined) playlist.description = change.description;
			if (change.pinned !== undefined) playlist.pinned = change.pinned;
		}
		playlist.trackCount = this.items.length;
		playlist.durationUs = this.items.reduce((us, i) => us + (i.track ?? i.external)!.durationUs, 0);
		if (this.items.length === 0) playlist.image = null;
		this.playlist = playlist;
	}
}

/**
 * A playlist and every item, at one version, so positions and `baseVersion` agree. Tries again
 * when the playlist changes while it loads.
 */
export async function loadPlaylist(
	fetch: Fetch,
	libraryId: string,
	playlistId: string
): Promise<LoadedPlaylist> {
	for (let attempt = 1; ; attempt++) {
		try {
			const items: PlaylistItem[] = [];
			let version: number | undefined;
			let cursor: string | undefined;
			do {
				const page = await listPlaylistItems(fetch, libraryId, playlistId, { limit: 1000, cursor });
				version ??= page.version;
				items.push(...page.items);
				cursor = page.nextCursor ?? undefined;
			} while (cursor);
			const playlist = await getPlaylist(fetch, libraryId, playlistId);
			if (playlist.version === version) return { playlist, items };
		} catch (e) {
			// `cursor_expired`: it changed between pages.
			if (!(e instanceof ApiError && e.status === 410)) throw e;
		}
		if (attempt === 3) throw new Error('The playlist kept changing while it loaded.');
	}
}

/** Whether `e` is the server out of reach or overloaded, rather than refusing the edit. */
function unreachable(e: unknown): boolean {
	if (e instanceof ApiError) return e.status >= 500 || e.code === 'rate_limited';
	// `fetch` rejects with a TypeError when there is no response at all.
	return e instanceof TypeError;
}

function retryDelay(e: unknown, attempt: number): number {
	if (e instanceof ApiError && e.retryAfter) return e.retryAfter * 1000;
	return Math.min(MAX_RETRY_MS, 1000 * 2 ** attempt);
}
