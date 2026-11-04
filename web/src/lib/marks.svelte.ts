// Which tracks the user likes and which they hid, by library, as every list shows them
// (`requirements/playlists.md` §11, `requirements/conventions.md` §8). A track is read with
// `liked`; a like, an unlike, a hide or an unhide made here, or one the server tells of from
// another device, holds from then on over what was read. A change here shows at once and is sent
// after, and one the server refuses is undone, with why (`requirements/general.md` §3.4).

import { SvelteMap } from 'svelte/reactivity';
import { ApiError, hideTrack, likeTrack, unhideTrack, unlikeTrack } from './api/client';
import type { RealtimeServerMessage } from './api/types';
import { realtime } from './realtime.svelte';
import { toast } from './toast.svelte';

type Fetch = typeof fetch;

/** What is known of a track since it was read. */
type Mark = { liked?: boolean; hidden?: boolean };

/** A track as the marks need it. */
type Marked = { id: string; title: string; liked?: boolean };

class Marks {
	/** By `{library}/{track}`. */
	#marks = new SvelteMap<string, Mark>();
	/**
	 * The latest change sent for each track, by `{library}/{track}`. While one is under way, the
	 * server's word on that track may be the echo of an older change this device made, so it is
	 * recorded but not shown. Nothing shows this, so nothing reacts to it.
	 */
	#sending = new Map<string, number>();
	#sent = 0;
	/**
	 * What the server has confirmed of each track, by `{library}/{track}`: the changes it took and
	 * what it told of. A refusal goes back to this, never to a change still unconfirmed.
	 */
	#confirmed = new Map<string, Mark>();

	/** Whether `track` of `library` is liked: as it was read, unless a change since says otherwise. */
	liked(library: string, track: Pick<Marked, 'id' | 'liked'>): boolean {
		return this.#marks.get(key(library, track.id))?.liked ?? track.liked ?? false;
	}

	/** Whether the user hid track `id` of `library`; a track read in a list was not. */
	hidden(library: string, id: string): boolean {
		return this.#marks.get(key(library, id))?.hidden ?? false;
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

	/** Takes what the server says changed on another device, or on this one. */
	receive(message: RealtimeServerMessage) {
		if (message.type === 'trackLiked') {
			const { libraryId, trackId, liked } = message;
			this.#confirm(key(libraryId, trackId), liked ? { liked, hidden: false } : { liked });
		} else if (message.type === 'trackHidden') {
			const { libraryId, trackId, hidden } = message;
			this.#confirm(key(libraryId, trackId), hidden ? { hidden, liked: false } : { hidden });
		}
	}

	/** Forgets every change, so each test starts afresh. */
	clear() {
		this.#marks.clear();
		this.#sending.clear();
		this.#confirmed.clear();
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
		} catch (e) {
			if (this.#sending.get(at) === sent) {
				const confirmed = this.#confirmed.get(at);
				if (confirmed) this.#marks.set(at, confirmed);
				else this.#marks.delete(at);
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
		this.#marks.set(at, { ...this.#marks.get(at), ...mark });
	}
}

const key = (library: string, track: string) => `${library}/${track}`;

export const marks = new Marks();
realtime.on((message) => marks.receive(message));
