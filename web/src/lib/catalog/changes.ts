// What the device's copy has changed, counted, so a read can later ask whether anything it
// depends on changed since it began (`live.svelte.ts`).

import type { Kind } from './kinds';

/** A count of the changes announced, and for each library and kind the count of the last. */
export class Changes {
	#count = 0;
	#last = new Map<string, number>();

	/** Counts so far; take it before a read, and give it to `since` to learn what changed after. */
	get count() {
		return this.#count;
	}

	/** Notes that `library` changed in `kinds`. */
	record(library: string, kinds: readonly Kind[]) {
		this.#count++;
		for (const kind of kinds) this.#last.set(`${library}/${kind}`, this.#count);
	}

	/** Whether `library` changed in any of `kinds` after `count` was taken. */
	since(count: number, library: string, kinds: readonly Kind[]) {
		return kinds.some((kind) => (this.#last.get(`${library}/${kind}`) ?? 0) > count);
	}
}
