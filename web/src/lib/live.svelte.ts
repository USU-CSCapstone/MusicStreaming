// What a page showing a library's catalog does when the device's copy changes under it: read
// again, in place, once per burst of changes (`catalog/client.svelte.ts` announces them).

import { createSubscriber } from 'svelte/reactivity';
import { catalog } from './catalog/client.svelte';
import type { Kind } from './catalog/kinds';

/** How long a burst of announcements is gathered into one reload, and the least between two. */
export const RELOAD_MS = 1000;

/**
 * The kinds of change each shown list depends on, which `reloadOnChange` reloads for and ignores
 * the rest. A list that shows tracks depends on what a track row carries: its album and artists,
 * its tags, whether it is hidden, and whether it is liked.
 */
export const DEPENDS = {
	tracks: ['tracks', 'albums', 'artists', 'tags', 'hidden', 'liked'],
	albums: ['albums', 'artists', 'tracks', 'tags', 'hidden'],
	artists: ['artists', 'albums', 'tracks', 'tags', 'hidden'],
	tags: ['tags', 'tracks', 'hidden'],
	playlists: ['playlists', 'tracks', 'hidden'],
	playlist: ['playlists', 'tracks', 'albums', 'artists', 'tags', 'hidden', 'liked']
} as const satisfies Record<string, readonly Kind[]>;

/**
 * Whether two reads are the same rows, so a reload that found nothing new can leave what is shown,
 * and what the user selected in it, alone.
 */
export const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/**
 * Calls `reload` once for a burst of announcements about `library` that name a kind in `depends`,
 * `RELOAD_MS` after the first, and once more after a reload that more announcements arrived
 * during. A change announced after `since` (`catalog.changes` as it was before the read being
 * shown) and before this listens counts as the first of a burst, so a read that began before the
 * page showed it is not left stale. A failed reload is dropped: the page keeps what it shows,
 * and the next announcement tries again. Returns the way to stop listening.
 */
export function reloadOnChange(
	library: string,
	depends: readonly Kind[],
	reload: () => Promise<void>,
	since: number
): () => void {
	let timer: ReturnType<typeof setTimeout> | undefined;
	let reloading = false;
	let again = false;
	let stopped = false;
	const schedule = () => {
		timer = setTimeout(async () => {
			timer = undefined;
			reloading = true;
			try {
				await reload();
			} catch {
				// Nothing to tell the user: what is shown is still true, only older.
			}
			reloading = false;
			if (again && !stopped) {
				again = false;
				schedule();
			}
		}, RELOAD_MS);
	};
	const stop = catalog.onChanged((changed, kinds) => {
		if (changed !== library || !kinds.some((kind) => depends.includes(kind))) return;
		if (reloading) again = true;
		else if (timer === undefined) schedule();
	});
	if (catalog.changedSince(since, library, depends)) schedule();
	return () => {
		stopped = true;
		stop();
		clearTimeout(timer);
	};
}

/** One thing read from a library that reads again, in place, when the library changes. */
export class Live<T> {
	#value: T;
	#subscribe: () => void;

	/**
	 * Shows `initial` and reads again with `read` when `library` changes in a kind of `depends`,
	 * while something displays `value`. `since` is `catalog.changes` taken before `initial` was
	 * read. The arguments run as `Paged`'s do.
	 */
	constructor(
		initial: T,
		read: () => Promise<T>,
		library: string,
		depends: readonly Kind[],
		since: number
	) {
		this.#value = $state.raw(initial);
		this.#subscribe = createSubscriber(() =>
			reloadOnChange(
				library,
				depends,
				async () => {
					const next = await read();
					if (!same(next, this.#value)) this.#value = next;
				},
				since
			)
		);
	}

	/** What was last read; reading it in an effect makes it listen for the library changing. */
	get value(): T {
		this.#subscribe();
		return this.#value;
	}
}
