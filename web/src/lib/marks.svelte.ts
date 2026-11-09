// Which tracks the user likes and which they hid, by library, as every list shows them
// (`requirements/playlists.md` §11, `requirements/conventions.md` §8). A track is read with
// `liked`; a like, an unlike, a hide or an unhide made here, or one the server tells of from
// another device, holds from then on over what was read. A change here shows at once and is sent
// after, and one the server refuses is undone, with why (`requirements/general.md` §3.4). The
// server tells nothing of what changed while the connection was down, so each time it comes
// back, every track marked here takes the server's word on it again.

import { SvelteMap, SvelteSet } from 'svelte/reactivity';
import {
	ApiError,
	getTracksByIds,
	hideTrack,
	likeTrack,
	listHiddenTracks,
	unhideTrack,
	unlikeTrack
} from './api/client';
import type { Playlist, RealtimeServerMessage } from './api/types';
import { realtime } from './realtime.svelte';
import { toast } from './toast.svelte';

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
	 * The latest change sent for each track, by `{library}/{track}`. While one is under way, the
	 * server's word on that track may be the echo of an older change this device made, so it is
	 * recorded but not shown. Nothing shows this, so nothing reacts to it.
	 */
	#sending = new Map<string, number>();
	#sent = 0;
	/**
	 * When each mark was last written, by `{library}/{track}`, counting every write, so a check of
	 * the server's word can tell a mark written while it was under way. Nothing reacts to it.
	 */
	#written = new Map<string, number>();
	#writes = 0;
	/**
	 * What the server has confirmed of each track, by `{library}/{track}`: the changes it took and
	 * what it told of. A refusal goes back to this, never to a change still unconfirmed.
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
	like(f: Fetch, library: string, track: Marked, liked: boolean): Promise<void> {
		const mark = liked ? { liked, hidden: false } : { liked };
		const send = liked ? likeTrack : unlikeTrack;
		const what = liked ? 'like' : 'unlike';
		return this.#change(library, track, mark, () => send(f, library, track.id), what);
	}

	/** Hides `track`, which unlikes it, or unhides it. */
	hide(f: Fetch, library: string, track: Marked, hidden: boolean): Promise<void> {
		const mark = hidden ? { hidden, liked: false } : { hidden };
		const send = hidden ? hideTrack : unhideTrack;
		const what = hidden ? 'hide' : 'unhide';
		return this.#change(library, track, mark, () => send(f, library, track.id), what);
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
		this.#sending.clear();
		this.#written.clear();
		this.#confirmed.clear();
		this.#cleared.clear();
		this.#likedSongs.clear();
	}

	/**
	 * Shows `mark` on `track` at once and sends it. A refusal of the latest change to the track goes
	 * back to what the server has confirmed of it, and says why; one of an earlier change leaves the
	 * later one showing, and its own answer decides.
	 */
	async #change(
		library: string,
		track: Marked,
		mark: Mark,
		send: () => Promise<void>,
		what: string
	) {
		const at = key(library, track.id);
		const sent = ++this.#sent;
		this.#sending.set(at, sent);
		this.#merge(at, mark);
		try {
			await send();
			this.#confirmed.set(at, { ...this.#confirmed.get(at), ...mark });
			// What the server now holds, over anything that wrote the mark meanwhile, such as Liked
			// Songs deleted elsewhere, whose echo this change held back; a later change decides
			// for itself.
			if (this.#sending.get(at) === sent) this.#merge(at, mark);
		} catch (e) {
			if (this.#sending.get(at) === sent) {
				const confirmed = this.#confirmed.get(at);
				this.#put(at, confirmed);
				// Shown again as the server last confirmed it, or else as it was read.
				if (mark.hidden !== undefined) {
					this.#tell(library, track.id, confirmed?.hidden ?? track.hidden ?? false);
				}
			}
			toast(`Couldn’t ${what} “${track.title}”. ${e instanceof ApiError ? e.message : ''}`.trim());
		} finally {
			if (this.#sending.get(at) === sent) this.#sending.delete(at);
		}
	}

	/**
	 * Takes `mark` as the server's word on the track at `at`. It shows unless a change of this
	 * device's is under way there, whose echo it may be; that change's own answer decides.
	 */
	#confirm(at: string, mark: Mark) {
		this.#confirmed.set(at, { ...this.#confirmed.get(at), ...mark });
		if (!this.#sending.has(at)) this.#merge(at, mark);
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
	 * Has each track marked here, with no change of this device's under way, take the server's word
	 * on whether it is liked, read with the track, and hidden, from Hidden Songs. A track neither
	 * returns is gone, and so neither. A mark written while this is under way is newer, so stays.
	 * A library that cannot be read keeps its marks until the next time.
	 */
	async #recheck(f: Fetch) {
		const since = this.#writes;
		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- the check's own, not shown
		const libraries = new Map<string, string[]>();
		for (const at of this.#marks.keys()) {
			if (this.#sending.has(at)) continue;
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
						const page = await listHiddenTracks(f, library, { limit: 1000, cursor });
						for (const track of page.items) hidden.add(track.id);
						cursor = page.nextCursor ?? undefined;
					} while (cursor);
					// eslint-disable-next-line svelte/prefer-svelte-reactivity -- the check's own, not shown
					const liked = new Set<string>();
					for (let i = 0; i < tracks.length; i += 100) {
						const page = await getTracksByIds(f, library, tracks.slice(i, i + 100));
						for (const track of page.items) if (track.liked) liked.add(track.id);
					}
					for (const track of tracks) {
						const at = key(library, track);
						if (this.#sending.has(at) || (this.#written.get(at) ?? 0) > since) continue;
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

export const marks = new Marks();
realtime.on((message) => void marks.receive(message));
