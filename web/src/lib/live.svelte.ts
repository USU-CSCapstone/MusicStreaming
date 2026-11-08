// What a page showing a library's catalog does when the device's copy changes under it: read
// again, in place, once per burst of changes (`catalog/client.svelte.ts` announces them).

import { createSubscriber } from 'svelte/reactivity';
import { catalog } from './catalog/client.svelte';

/** How long a burst of announcements is gathered into one reload, and the least between two. */
export const RELOAD_MS = 1000;

/** Whether two reads are the same rows, so a reload that found nothing new can leave what is shown, and what the user selected in it, alone. */
export const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/**
 * Calls `reload` once for a burst of announcements about `library`, `RELOAD_MS` after the first,
 * and once more after a reload that more announcements arrived during. A failed reload is
 * dropped: the page keeps what it shows, and the next announcement tries again. Returns the way
 * to stop listening.
 */
export function reloadOnChange(library: string, reload: () => Promise<void>): () => void {
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
	const stop = catalog.onChanged((changed) => {
		if (changed !== library) return;
		if (reloading) again = true;
		else if (timer === undefined) schedule();
	});
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
	 * Shows `initial` and reads again with `read` when `library` changes, while something
	 * displays `value`.
	 */
	constructor(initial: T, library: string, read: () => Promise<T>) {
		this.#value = $state.raw(initial);
		this.#subscribe = createSubscriber(() =>
			reloadOnChange(library, async () => {
				const next = await read();
				if (!same(next, this.#value)) this.#value = next;
			})
		);
	}

	/** What was last read; reading it in an effect makes it listen for the library changing. */
	get value(): T {
		this.#subscribe();
		return this.#value;
	}
}
