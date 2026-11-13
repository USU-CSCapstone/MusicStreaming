// Which tracks the user likes and which they hid, by library, as every list shows them
// (`requirements/playlists.md` §11, `requirements/conventions.md` §8). A track is read with
// `liked`; a like, an unlike, a hide or an unhide made here, or one the server tells of from
// another device, holds from then on over what was read.
//
// A change made here is a mutation (`sync/mutations/marks.ts`), which the device's copy makes and
// the sender sends. This overlay stays beside the device because not every list reads the device
// at once: a library read from the server (not on the device, still downloading, or with no room)
// shows a change only through it (`requirements/general.md` §3.4), a list read before reloads up
// to a second after the device changes, and the server's word on another device's change reaches
// lists already read only here. It can go once every list reads the device and reloads on its
// changes. A change the server refuses is undone here, as the device takes it back, with why. The
// server tells nothing of what changed while the connection was down, so each time it comes back,
// every track marked here that is not waiting to be sent takes the server's word on it again.

import { SvelteMap, SvelteSet } from 'svelte/reactivity';
import { getTracksByIds, listHiddenTracks } from './api/client';
import type { Playlist, RealtimeServerMessage } from './api/types';
import { realtime } from './realtime.svelte';
import { mintId } from './sync/mutations/marks';
import { sender } from './sync/sender';

type Fetch = typeof fetch;

/** What is known of a track since it was read. */
type Mark = { liked?: boolean; hidden?: boolean };

/**
 * A track as the marks need it, with `liked` and `hidden` as it was read. A list shows no hidden
 * track, so `hidden` is left out but where it is one, as on Hidden Songs.
 */
type Marked = { id: string; title: string; liked?: boolean; hidden?: boolean };

/** Hears that a track was hidden or unhidden, as the marks show it. */
export type HiddenListener = (library: string, track: string, hidden: boolean) => void;

class Marks {
	/** By `{library}/{track}`. */
	#marks = new SvelteMap<string, Mark>();
	/**
	 * Each mark's `hidden`, by library and then track, kept with `#marks`. Lists read hides from
	 * here, so a like, which lists do not filter by, never makes them filter again.
	 */
	#hides = new SvelteMap<string, SvelteMap<string, boolean>>();
	/**
	 * The latest change made here to each track that is unsent, by `{library}/{track}`. While one
	 * is, the server's word on that track may be the echo of an older change this device made, so it
	 * is recorded but not shown. Nothing shows this, so nothing reacts to it.
	 */
	#pending = new Map<string, object>();
	/**
	 * What a track was read as, hidden or not, for the change under way, which a refusal goes back
	 * to.
	 */
	#read = new Map<string, boolean>();
	/**
	 * When each mark was last written, by `{library}/{track}`, counting every write, so a check of
	 * the server's word can tell a mark written while it was under way. Nothing reacts to it.
	 */
	#written = new Map<string, number>();
	#writes = 0;
	/**
	 * What the server has said of each track, by `{library}/{track}`. A refusal goes back to this,
	 * never to a change still unsent.
	 */
	#confirmed = new Map<string, Mark>();
	/** Libraries whose Liked Songs was deleted since tracks were read, which unliked them all. */
	#cleared = new SvelteSet<string>();
	/** Each library's Liked Songs, by ID, as seen, so its deletion elsewhere is known. */
	#likedSongs = new Map<string, string>();
	/** Who hears of each hide and unhide shown, such as the queue, which leaves hidden rows out. */
	#listeners = new Set<HiddenListener>();

	/**
	 * Calls `listener` with each hide and unhide as it shows, made here or heard of, and each one
	 * undone; returns how to stop.
	 */
	onHidden(listener: HiddenListener): () => void {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	}

	/** Whether `track` of `library` is liked: as it was read, unless a change since says otherwise. */
	liked(library: string, track: Pick<Marked, 'id' | 'liked'>): boolean {
		const read = !this.#cleared.has(library) && (track.liked ?? false);
		return this.#marks.get(key(library, track.id))?.liked ?? read;
	}

	/**
	 * Unlikes every track of `library`, as deleting its Liked Songs did. Each track liked since
	 * reads as liked again only through a like this store hears of.
	 */
	unlikeAll(library: string) {
		this.#cleared.add(library);
		for (const [at, mark] of [...this.#marks]) {
			if (at.startsWith(`${library}/`) && mark.liked) this.#put(at, { ...mark, liked: false });
		}
		for (const [at, mark] of this.#confirmed) {
			if (at.startsWith(`${library}/`) && mark.liked)
				this.#confirmed.set(at, { ...mark, liked: false });
		}
	}

	/** Notes which of `playlists` is a library's Liked Songs, so its deletion elsewhere is known. */
	note(playlists: readonly Pick<Playlist, 'id' | 'libraryId' | 'kind'>[]) {
		for (const p of playlists) if (p.kind === 'liked') this.#likedSongs.set(p.libraryId, p.id);
	}

	/**
	 * Whether the user hid track `id` of `library`: as it was read, `read`, unless a change since
	 * says otherwise. A track read in a list was not; a playlist's item says.
	 */
	hidden(library: string, id: string, read = false): boolean {
		return this.#hides.get(library)?.get(id) ?? read;
	}

	/**
	 * `tracks` of `library` but those the user hid since they were read, which no list shows:
	 * `tracks` itself while that leaves none out, so a list stays the same list, and a selection
	 * or a drag in it carries on, as songs elsewhere are hidden.
	 */
	unhidden<T extends { id: string }>(library: string, tracks: T[]): T[] {
		const hides = this.#hides.get(library);
		if (!hides) return tracks;
		const shown = tracks.filter((track) => !(hides.get(track.id) ?? false));
		return shown.length === tracks.length ? tracks : shown;
	}

	/** Likes `track`, which unhides it, or unlikes it. */
	like(library: string, track: Marked, liked: boolean): Promise<void> {
		const mark = liked ? { liked, hidden: false } : { liked };
		return this.#change(library, track, mark, liked ? 'like' : 'unlike');
	}

	/** Hides `track`, which unlikes it, or unhides it. */
	hide(library: string, track: Marked, hidden: boolean): Promise<void> {
		const mark = hidden ? { hidden, liked: false } : { hidden };
		return this.#change(library, track, mark, hidden ? 'hide' : 'unhide');
	}

	/**
	 * Takes what the server says changed on another device, or on this one. A `hello`, sent as the
	 * connection opens again, has every marked track checked with the server, through `f`; the
	 * promise settles once that is done.
	 */
	async receive(message: RealtimeServerMessage, f: Fetch = fetch): Promise<void> {
		if (message.type === 'hello') {
			await this.#recheck(f);
		} else if (message.type === 'trackLiked') {
			const { libraryId, trackId, liked } = message;
			this.#confirm(key(libraryId, trackId), liked ? { liked, hidden: false } : { liked });
		} else if (message.type === 'trackHidden') {
			const { libraryId, trackId, hidden } = message;
			this.#confirm(key(libraryId, trackId), hidden ? { hidden, liked: false } : { hidden });
		} else if (message.type === 'playlist') {
			const { libraryId, playlistId, playlist } = message;
			if (playlist) this.note([playlist]);
			else if (this.#likedSongs.get(libraryId) === playlistId) this.unlikeAll(libraryId);
		}
	}

	/** Forgets every change, so each test starts afresh. */
	clear() {
		this.#marks.clear();
		this.#hides.clear();
		this.#pending.clear();
		this.#read.clear();
		this.#written.clear();
		this.#confirmed.clear();
		this.#cleared.clear();
		this.#likedSongs.clear();
	}

	/**
	 * Shows `mark` on `track` at once and keeps it as a mutation, which the device makes at once and
	 * the sender sends, now or when the device is online again. Settles once it is sent or refused,
	 * or left for another tab to send. A refused change goes back to what the server last said of
	 * the track, or else as it was read.
	 */
	async #change(library: string, track: Marked, mark: Mark, what: string) {
		const at = key(library, track.id);
		this.#merge(at, mark);
		if (!this.#pending.has(at)) this.#read.set(at, track.hidden ?? false);
		const queued = sender.enqueue('mark', {
			...mark,
			library,
			track: track.id,
			what: `${what} “${track.title}”`,
			likedId: mark.liked ? mintId() : undefined
		});
		if (!queued) return;
		// Its own token: a later change may be folded into the same mutation.
		const change = {};
		this.#pending.set(at, change);
		const outcome = await sender.settled(queued.key);
		if (outcome === 'sent') this.#confirmed.set(at, { ...this.#confirmed.get(at), ...mark });
		// A later change to the track decides for itself.
		if (this.#pending.get(at) !== change) return;
		this.#pending.delete(at);
		// What the server now holds, over anything that wrote the mark meanwhile, such as Liked Songs
		// deleted elsewhere, whose echo it held back.
		if (outcome === 'sent') this.#merge(at, mark);
		if (outcome === 'dropped') {
			const confirmed = this.#confirmed.get(at);
			this.#put(at, confirmed);
			// Shown again as the server last said, or else as it was read.
			if (mark.hidden !== undefined)
				this.#tell(library, track.id, confirmed?.hidden ?? this.#read.get(at) ?? false);
		}
		this.#read.delete(at);
	}

	/**
	 * Takes `mark` as the server's word on the track at `at`. It shows unless a change of this
	 * device's is unsent there, whose echo it may be; that change's own answer decides.
	 */
	#confirm(at: string, mark: Mark) {
		this.#confirmed.set(at, { ...this.#confirmed.get(at), ...mark });
		if (!this.#pending.has(at)) this.#merge(at, mark);
	}

	#merge(at: string, mark: Mark) {
		const was = this.#marks.get(at);
		this.#put(at, { ...was, ...mark });
		if (mark.hidden !== undefined && mark.hidden !== was?.hidden) {
			const [library, track] = split(at);
			this.#tell(library, track, mark.hidden);
		}
	}

	/**
	 * Has each track marked here, with no change of this device's unsent, take the server's word
	 * on whether it is liked, read with the track, and hidden, from Hidden Songs. Both are read from
	 * the server: the device's copy is behind until the catch-up this connection starts finishes. A
	 * track neither returns is gone, and so neither. A mark written while this is under way is
	 * newer, so stays.
	 * A library that cannot be read keeps its marks until the next time.
	 */
	async #recheck(f: Fetch) {
		const since = this.#writes;
		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- the check's own, not shown
		const libraries = new Map<string, string[]>();
		for (const at of this.#marks.keys()) {
			if (this.#pending.has(at)) continue;
			const [library, track] = split(at);
			libraries.set(library, [...(libraries.get(library) ?? []), track]);
		}
		await Promise.all(
			[...libraries].map(async ([library, tracks]) => {
				try {
					// eslint-disable-next-line svelte/prefer-svelte-reactivity -- the check's own, not shown
					const hidden = new Set<string>();
					let cursor: string | undefined;
					do {
						const page = await listHiddenTracks(f, library, { limit: 1000, cursor }, true);
						for (const track of page.items) hidden.add(track.id);
						cursor = page.nextCursor ?? undefined;
					} while (cursor);
					// eslint-disable-next-line svelte/prefer-svelte-reactivity -- the check's own, not shown
					const liked = new Set<string>();
					for (let i = 0; i < tracks.length; i += 100) {
						const page = await getTracksByIds(f, library, tracks.slice(i, i + 100), true);
						for (const track of page.items) if (track.liked) liked.add(track.id);
					}
					for (const track of tracks) {
						const at = key(library, track);
						if (this.#pending.has(at) || (this.#written.get(at) ?? 0) > since) continue;
						const mark = { liked: liked.has(track), hidden: hidden.has(track) };
						this.#confirmed.set(at, mark);
						this.#merge(at, mark);
					}
				} catch {
					// Kept as they are; the next connection checks again.
				}
			})
		);
	}

	/** Sets the mark at `at`, or with none forgets it, and its hide with it. */
	#put(at: string, mark: Mark | undefined) {
		this.#written.set(at, ++this.#writes);
		if (mark) this.#marks.set(at, mark);
		else this.#marks.delete(at);
		const [library, track] = split(at);
		const hides = this.#hides.get(library);
		if (mark?.hidden === undefined) hides?.delete(track);
		else if (hides) hides.set(track, mark.hidden);
		else this.#hides.set(library, new SvelteMap([[track, mark.hidden]]));
	}

	#tell(library: string, track: string, hidden: boolean) {
		for (const listener of this.#listeners) listener(library, track, hidden);
	}
}

const key = (library: string, track: string) => `${library}/${track}`;

/** The library and the track of a key. */
function split(at: string): [string, string] {
	const slash = at.indexOf('/');
	return [at.slice(0, slash), at.slice(slash + 1)];
}

/** The one set of marks the app shows, fed by realtime messages and the changes made here. */
export const marks = new Marks();
realtime.on((message) => void marks.receive(message));
