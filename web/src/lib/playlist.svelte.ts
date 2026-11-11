// One playlist as the user edits it. Every edit shows at once and the server confirms it
// afterwards (`requirements/general.md` §3.4, `requirements/playlists.md` §4–§5).
//
// An edit is a mutation (`sync/mutations/playlists.ts`): the device's copy makes it at once, and
// the sender keeps it through a closed app and sends it, after the edits made before it. This page
// shows it at once itself, and then what the device reads back, which holds every edit not yet
// sent, wherever it was made: a playlist opened again after the app was closed shows them as any
// other. An edit the server refuses is taken back off the device with every edit made on it, the
// user is told, and the page shows the device's playlist again.

import type { Playlist, PlaylistItem, PlaylistOp, TrackSummary } from './api/types';
import { fingerprint, toPlayable } from './sync/fingerprint';
import {
	type LoadedPlaylist,
	type PlaylistChange,
	CONFLICT_MESSAGE,
	loadPlaylist,
	playlistScope
} from './sync/mutations/playlists';
import { sender } from './sync/sender';
import { type Resolve, applyOps, invertOps, onlyNew } from './playlist-ops';

export { CONFLICT_MESSAGE, loadPlaylist };
export type { LoadedPlaylist, PlaylistChange };

/** The signature of `fetch`. */
type Fetch = typeof fetch;

/** `ops` and `resolve` as they apply to `items` of a Liked Songs if `liked`, else as they are. */
const rule = (
	liked: boolean,
	items: readonly PlaylistItem[],
	ops: PlaylistOp[],
	resolve: Resolve<PlaylistItem>
) => (liked ? onlyNew(items, ops, resolve, (item) => item.track?.id) : { ops, resolve });

/**
 * A playlist as shown: what the device (or, where the library is not on it, the server) read, with
 * the edits made here since applied at once.
 */
export class EditablePlaylist {
	/** As shown. */
	playlist = $state<Playlist>()!;
	/** Every item, in order; an item's index is its position for edits. */
	items = $state<PlaylistItem[]>([]);

	#fetch: Fetch;
	/** The keys of this playlist's mutations the sender holds unsent; reassigned, so reactive. */
	#unsent = $state.raw<string[]>([]);
	/** Edits made here that the device has not made yet, or found it cannot. */
	#applying = 0;
	/** The newest playlist `refresh` was given while an edit was on its way to the device. */
	#later: LoadedPlaylist | null = null;
	/** A change to it was refused: what is read next is shown, whatever its version. */
	#reset = false;
	#added = $state(false);

	/** Shows `loaded`, which is read with `fetch` again after a refusal. */
	constructor(fetch: Fetch, loaded: LoadedPlaylist) {
		this.#fetch = fetch;
		this.playlist = loaded.playlist;
		this.items = loaded.items;
		for (const key of sender.pendingIn(playlistScope(loaded.playlist.id))) this.#follow(key);
	}

	/** Whether there are changes to it the server has not confirmed yet. */
	get saving(): boolean {
		return this.#unsent.length > 0;
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
	 * Applies `change` here at once, and keeps it to send. Adds name tracks, whose summaries
	 * `tracks` gives, so what shows here is exactly what the server stores: expand an album or
	 * artist before adding it. A position out of range throws, and nothing applies.
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
	 * Applies, keeps and sends `change` like `edit`, with `resolve` naming what its adds bring in;
	 * returns its undo.
	 */
	#edit(change: PlaylistChange, resolve: Resolve<PlaylistItem>): () => void {
		const before = { playlist: this.playlist, items: this.items };
		const { ops } = change;
		const liked = this.playlist.kind === 'liked';
		// Worked out once to apply and once to invert: a resolver names what it adds once.
		const applying = ops && rule(liked, before.items, ops, resolve);
		this.items = applying ? applyOps(before.items, applying.ops, applying.resolve) : before.items;
		if (ops?.some((op) => op.op === 'add')) this.#added = true;
		this.#present(change);

		const adds = ops?.flatMap((op) => (op.op === 'add' ? [op.items] : [])) ?? [];
		const queued = sender.enqueue('editPlaylist', {
			libraryId: before.playlist.libraryId,
			playlistId: before.playlist.id,
			baseVersion: before.playlist.version,
			change,
			fingerprint: fingerprint(before.items),
			tracks: adds.every((items) => items.every((p) => p.type === 'track'))
				? adds.map((items) => items.map((p) => (p as { id: string }).id))
				: undefined
		});
		if (queued) {
			this.#applying++;
			void queued.applied.finally(() => {
				this.#applying--;
				const later = this.#later;
				this.#later = null;
				if (later && this.#applying === 0) this.refresh(later);
			});
			this.#follow(queued.key);
		}

		const undo: PlaylistChange = {};
		if (change.title !== undefined) undo.title = before.playlist.title;
		if (change.description !== undefined) undo.description = before.playlist.description;
		if (change.pinned !== undefined) undo.pinned = before.playlist.pinned;
		const applied = ops && rule(liked, before.items, ops, resolve);
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
	 * what this shows: an edit made here shows until the device has made it too, and a refusal has
	 * the next read shown whatever its version, as it takes edits back.
	 */
	refresh(loaded: LoadedPlaylist) {
		if (this.#applying > 0) {
			this.#later = loaded;
			return;
		}
		if (!this.#reset && loaded.playlist.version <= this.playlist.version) return;
		this.#reset = false;
		this.playlist = loaded.playlist;
		this.items = loaded.items;
	}

	/** Settles once every change to it this tab holds is confirmed or refused. */
	async idle(): Promise<void> {
		await Promise.all(this.#unsent.map((key) => sender.settled(key)));
	}

	/**
	 * Follows the mutation `key` of this playlist until it settles; one refused has the playlist
	 * read again, as the device has it once its changes are taken back.
	 */
	#follow(key: string) {
		this.#unsent = [...this.#unsent, key];
		void sender.settled(key).then(async (outcome) => {
			this.#unsent = this.#unsent.filter((k) => k !== key);
			if (outcome !== 'dropped') return;
			this.#reset = true;
			const { libraryId, id } = this.playlist;
			try {
				this.refresh(await loadPlaylist(this.#fetch, libraryId, id));
			} catch {
				// The next read the library's changes bring shows it.
			}
		});
	}

	/**
	 * The details after `change`, at the version the edit makes. Totals count what shows here,
	 * which holds every item; the artwork waits for the server, except that an empty playlist has
	 * none.
	 */
	#present(change: PlaylistChange) {
		const playlist = { ...this.playlist, version: this.playlist.version + 1 };
		if (change.title !== undefined) playlist.title = change.title;
		if (change.description !== undefined) playlist.description = change.description;
		if (change.pinned !== undefined) playlist.pinned = change.pinned;
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
