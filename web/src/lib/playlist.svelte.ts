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
import type {
	Playable,
	Playlist,
	PlaylistEdit,
	PlaylistItem,
	PlaylistOp,
	TrackSummary
} from './api/types';
import { type Resolve, applyOps, invertOps, onlyNew } from './playlist-ops';
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

/**
 * A playlist as shown: the server's confirmed version with this page's pending edits applied at
 * once. The edits are kept and sent by the outbox, which tells of each answer.
 */
export class EditablePlaylist {
	/** As shown: what the server confirmed, with pending edits applied. */
	playlist = $state<Playlist>()!;
	/** Every item, in order; an item's index is its position for edits. */
	items = $state<PlaylistItem[]>([]);

	#fetch: Fetch;
	#confirmed: LoadedPlaylist;
	// Reassigned, never mutated, so `saving` is reactive.
	#queue = $state.raw<Pending[]>([]);
	#sending: Promise<void> | null = null;
	// Whether `#confirmed` is known to be the server's version, which the first edit makes sure of.
	#verified = false;
	/** The newest playlist `refresh` was given while edits were sending. */
	#later: LoadedPlaylist | null = null;
	#added = $state(false);

	/** Shows `loaded`, which is read with `fetch` again after a conflict. */
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
	 * Whether items were added here, an undo's included, since the playlist was loaded. Their
	 * dates added are this device's, and the server dates what it adds by its own clock when it
	 * saves the edit, so the two may order those items differently by date added.
	 */
	get added(): boolean {
		return this.#added;
	}

	/**
	 * Applies `change` here at once, and queues it for the server. Adds name tracks, whose
	 * summaries `tracks` gives, so what shows here is exactly what the server stores: expand an
	 * album or artist before adding it. A position out of range throws, and nothing applies.
	 *
	 * Returns its undo (`requirements/playlists.md` §4): a further edit reversing this one, which
	 * does nothing once the contents have changed again, as its positions would be wrong.
	 */
	edit(change: PlaylistChange, tracks: TrackSummary[] = []): () => void {
		// Only the timestamp is kept, so nothing needs to react to this date.
		// eslint-disable-next-line svelte/prefer-svelte-reactivity
		const addedAt = new Date().toISOString();
		return this.#edit(change, (added) =>
			added.map((p) => {
				const track = p.type === 'track' ? tracks.find((t) => t.id === p.id) : undefined;
				if (!track) throw new Error('Adding a track needs its summary.');
				return { track, addedAt };
			})
		);
	}

	/**
	 * Applies, queues and sends `change` like `edit`, with `resolve` naming what its adds bring in;
	 * returns its undo.
	 */
	#edit(change: PlaylistChange, resolve: Resolve<PlaylistItem>): () => void {
		const before = { playlist: this.playlist, items: this.items };
		const { ops } = change;
		// The server reshapes adds to a Liked Songs, so this does as it does, and both reach the
		// same items, at the same positions for the edits after.
		const liked = this.playlist.kind === 'liked';
		const rule = (items: readonly PlaylistItem[], ops: PlaylistOp[]) =>
			liked ? onlyNew(items, ops, resolve, (item) => item.track?.id) : { ops, resolve };
		const apply = (items: PlaylistItem[]) => {
			if (!ops) return items;
			const applied = rule(items, ops);
			return applyOps(items, applied.ops, applied.resolve);
		};

		this.items = apply(this.items);
		if (ops?.some((op) => op.op === 'add')) this.#added = true;
		this.#queue = [...this.#queue, { change, key: crypto.randomUUID(), apply }];
		this.#present();
		this.#sending ??= this.#send();

		const undo: PlaylistChange = {};
		if (change.title !== undefined) undo.title = before.playlist.title;
		if (change.description !== undefined) undo.description = before.playlist.description;
		if (change.pinned !== undefined) undo.pinned = before.playlist.pinned;
		const applied = ops && rule(before.items, ops);
		const inverse = applied
			? invertOps(before.items, applied.ops, applied.resolve, toPlayable)
			: { ops: [], resolve };
		if (inverse.ops.length) undo.ops = inverse.ops;
		const after = this.items;
		return () => {
			if (this.items !== after || Object.keys(undo).length === 0) return;
			// What comes back is what went: the same items, with the same dates added.
			this.#edit(undo, inverse.resolve);
		};
	}

	/**
	 * Shows `loaded`, read again after the library changed on this device, if it is newer than
	 * what this shows and nothing is pending here: an edit made here is never replaced, as the
	 * server's answer to it, or its conflict, comes first, and then the newest playlist held back
	 * is offered again.
	 */
	refresh(loaded: LoadedPlaylist) {
		if (this.#sending) {
			this.#later = loaded;
			return;
		}
		if (loaded.playlist.version <= this.#confirmed.playlist.version) return;
		this.#confirmed = loaded;
		this.items = loaded.items;
		this.#present();
	}

	/** Settles once every pending edit is confirmed, refused, or dropped. */
	async idle(): Promise<void> {
		while (this.#sending) await this.#sending;
	}

	/** Sends the queued edits one at a time, then offers `refresh` the playlist held back meanwhile. */
	async #send() {
		let attempt = 0;
		while (this.#queue.length > 0) {
			const next = this.#queue[0];
			try {
				if (!this.#verified) await this.#verify();
				const { playlist, items } = this.#confirmed;
				const confirmed = await editPlaylist(
					this.#fetch,
					playlist.libraryId,
					playlist.id,
					{ ...next.change, baseVersion: playlist.version },
					next.key
				);
				this.#confirmed = { playlist: confirmed, items: next.apply(items) };
				this.#queue = this.#queue.slice(1);
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
		const later = this.#later;
		this.#later = null;
		if (later) this.refresh(later);
	}

	/**
	 * Before the first edit: makes sure the playlist is the server's version, which a device
	 * that is behind does not give. Positions are the device's items', so if the server's items
	 * differ the edit conflicts, as another device's change would.
	 */
	async #verify() {
		const { playlist, items } = this.#confirmed;
		let server = await getPlaylist(this.#fetch, playlist.libraryId, playlist.id, true);
		if (server.version !== playlist.version) {
			const loaded = await loadPlaylist(this.#fetch, playlist.libraryId, playlist.id, true);
			const names = (list: PlaylistItem[]) => JSON.stringify(list.map(toPlayable));
			if (names(loaded.items) !== names(items)) {
				throw new ApiError(409, 'version_conflict', CONFLICT_MESSAGE);
			}
			server = loaded.playlist;
		}
		this.#confirmed = { playlist: server, items };
		this.#verified = true;
		this.#present();
	}

	/** After a conflict: the server's playlist, with nothing of this device's pending. */
	async #reload() {
		const { playlist } = this.#confirmed;
		try {
			this.#confirmed = await loadPlaylist(this.#fetch, playlist.libraryId, playlist.id, true);
			this.#verified = true;
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
 * Moves of the rows a list shows, the items `shows` says (by default all but external ones), as
 * moves of positions in the playlist. A row moved between two others goes in front of the one that
 * follows it, wherever items not shown lie. Each row move is made on the rows as the ones before
 * it left them, and so is each op.
 */
export function rowMoveOps(
	items: PlaylistItem[],
	moves: { indexes: number[]; to: number }[],
	shows: (item: PlaylistItem) => boolean = (item) => !!item.track
): PlaylistOp[] {
	const ops: PlaylistOp[] = [];
	for (const { indexes, to } of moves) {
		const shown = items.flatMap((item, position) => (shows(item) ? [position] : []));
		const positions = indexes.map((i) => shown[i]);
		const rest = shown.filter((p) => !positions.includes(p));
		// In front of the row that will follow them, or at the end.
		const next = rest[to];
		const at =
			next === undefined
				? items.length - positions.length
				: next - positions.filter((p) => p < next).length;
		const op: PlaylistOp = { op: 'move', positions, at };
		ops.push(op);
		items = applyOps(items, [op], () => []);
	}
	return ops;
}

/**
 * A playlist and every item, at one version, so positions and `baseVersion` agree. Tries again
 * when the playlist changes while it loads. With `fresh`, from the server alone, as the reload
 * after a conflict needs: a device behind the server would give the version that just conflicted.
 */
export async function loadPlaylist(
	fetch: Fetch,
	libraryId: string,
	playlistId: string,
	fresh = false
): Promise<LoadedPlaylist> {
	for (let attempt = 1; ; attempt++) {
		try {
			const items: PlaylistItem[] = [];
			let version: number | undefined;
			let cursor: string | undefined;
			do {
				const page = await listPlaylistItems(
					fetch,
					libraryId,
					playlistId,
					{ limit: 1000, cursor },
					fresh
				);
				version ??= page.version;
				items.push(...page.items);
				cursor = page.nextCursor ?? undefined;
			} while (cursor);
			const playlist = await getPlaylist(fetch, libraryId, playlistId, fresh);
			if (playlist.version === version) return { playlist, items };
		} catch (e) {
			// `cursor_expired`: it changed between pages.
			if (!(e instanceof ApiError && e.status === 410)) throw e;
		}
		if (attempt === 3) throw new Error('The playlist kept changing while it loaded.');
	}
}

/** How an edit names an item: a library track, or one from an external source. */
function toPlayable(item: PlaylistItem): Playable {
	if (item.track) return { type: 'track', id: item.track.id };
	return { type: 'external', sourceId: item.external!.source!, externalId: item.external!.id };
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
