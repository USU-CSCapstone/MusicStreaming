import { createSubscriber } from 'svelte/reactivity';
import type { Page } from './api/types';
import { reloadOnChange, same } from './live.svelte';

/** Rows a reload reads a page, the most the API gives. */
const RELOAD_PAGE = 1000;

/**
 * A cursor-paged list that grows as the user scrolls, and reads the window loaded again, in
 * place, when its library changes on the device while something displays it.
 */
export class Paged<T> {
	#items = $state<T[]>([]);
	#total = $state(0);
	#cursor = $state<string | null>(null);
	/** The read under way, which another waits for rather than runs beside. */
	#busy: Promise<void> | null = null;
	#subscribe: () => void;

	/** `load` reads the page after `cursor`, or the first page without one, of at most `limit` rows, or its usual size without one; `library` is the one it reads. */
	constructor(
		first: Page<T>,
		private load: (cursor?: string, limit?: number) => Promise<Page<T>>,
		library: string
	) {
		this.#items = first.items;
		this.#total = first.total;
		this.#cursor = first.nextCursor;
		this.#subscribe = createSubscriber(() => reloadOnChange(library, () => this.#reload()));
	}

	/** The rows loaded; reading them in an effect makes it listen for the library changing. */
	get items(): T[] {
		this.#subscribe();
		return this.#items;
	}

	/** How many rows the list has in all, as of the last read. */
	get total(): number {
		this.#subscribe();
		return this.#total;
	}

	get hasMore(): boolean {
		return this.#cursor !== null;
	}

	/** Reads the next page onto the end, or waits for the read under way; does nothing at the end of the list. */
	async more() {
		if (this.#busy) return this.#busy;
		if (this.#cursor === null) return;
		const cursor = this.#cursor;
		await this.#exclusive(async () => {
			const next = await this.load(cursor);
			this.#items.push(...next.items);
			this.#total = next.total;
			this.#cursor = next.nextCursor;
		});
	}

	/** Reads as many rows as are loaded from the start and swaps them in, so nothing jumps. */
	async #reload() {
		while (this.#busy) await this.#busy;
		await this.#exclusive(async () => {
			const rows: T[] = [];
			let page: Page<T>;
			let cursor: string | undefined;
			do {
				// No more rows than are shown, so an unchanged window reads back the same rows; an
				// empty list reads a first page of the usual size.
				const wanted = this.#items.length - rows.length;
				page = await this.load(cursor, wanted > 0 ? Math.min(RELOAD_PAGE, wanted) : undefined);
				rows.push(...page.items);
				cursor = page.nextCursor ?? undefined;
			} while (cursor && rows.length < this.#items.length);
			if (!same(rows, this.#items)) this.#items = rows;
			this.#total = page.total;
			this.#cursor = page.nextCursor;
		});
	}

	/** Runs `job` as the one read under way. */
	async #exclusive(job: () => Promise<void>) {
		const run = job();
		const done = () => {
			this.#busy = null;
		};
		this.#busy = run.then(done, done);
		await run;
	}

	/**
	 * An attachment for an element at the end of the list: loads pages while it is near
	 * the viewport, so a page too short to fill the screen still leads to the next. A list
	 * that scrolls inside an element of its own passes that element as `root`. It starts over
	 * when the list gains or loses its end, as a reload can push a row past it.
	 */
	sentinel = (el: Element, root: Element | null = null) => {
		void this.hasMore;
		let visible = false;
		let pumping = false;
		const pump = async () => {
			if (pumping) return;
			pumping = true;
			try {
				while (visible && this.hasMore) await this.more();
			} catch {
				// Leave the rest for the next time the end comes into view.
			} finally {
				pumping = false;
			}
		};
		const observer = new IntersectionObserver(
			(entries) => {
				visible = entries.some((e) => e.isIntersecting);
				void pump();
			},
			{ root, rootMargin: '800px' }
		);
		observer.observe(el);
		return () => observer.disconnect();
	};
}
