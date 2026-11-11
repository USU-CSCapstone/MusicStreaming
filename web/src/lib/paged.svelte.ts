import { createSubscriber } from 'svelte/reactivity';
import type { Page } from './api/types';
import type { Kind } from './catalog/kinds';
import { reloadOnChange, same } from './live.svelte';

/** Rows a reload reads a page, the most the API gives. */
const RELOAD_PAGE = 1000;

/**
 * A cursor-paged list that grows as the user scrolls, and reads the window loaded again, in
 * place, when its library changes on the device in a kind it depends on while something displays
 * it. A reload compares each page as it arrives and stops comparing at the first that differs, so
 * the rows before it keep their identity, and a list nothing changed keeps its array.
 */
export class Paged<T> {
	#items = $state.raw<T[]>([]);
	#total = $state(0);
	#cursor = $state<string | null>(null);
	/**
	 * Whether the list has rows past those loaded; it changes only when the list gains or loses its
	 * end.
	 */
	#more = $derived(this.#cursor !== null);
	/** The read under way, which another waits for rather than runs beside. */
	#busy: Promise<void> | null = null;
	#subscribe: () => void;

	/**
	 * `load` reads the page after `cursor`, or the first page without one, of at most `limit` rows,
	 * or its usual size without one; `library` is the one it reads, and `depends` the kinds of
	 * change that make it read again. `since` is `catalog.changes` taken before `first` was read.
	 */
	constructor(
		first: Page<T>,
		private load: (cursor?: string, limit?: number) => Promise<Page<T>>,
		library: string,
		depends: readonly Kind[],
		since: number
	) {
		this.#items = first.items;
		this.#total = first.total;
		this.#cursor = first.nextCursor;
		this.#subscribe = createSubscriber(() =>
			reloadOnChange(library, depends, () => this.#reload(), since)
		);
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
		return this.#more;
	}

	/**
	 * Reads the next page onto the end, or waits for the read under way; does nothing at the end of
	 * the list.
	 */
	async more() {
		if (this.#busy) return this.#busy;
		if (this.#cursor === null) return;
		const cursor = this.#cursor;
		await this.#exclusive(async () => {
			const next = await this.load(cursor);
			this.#items = [...this.#items, ...next.items];
			this.#total = next.total;
			this.#cursor = next.nextCursor;
		});
	}

	/**
	 * Reads as many rows as are loaded from the start and swaps them in, so nothing jumps. Each
	 * page is compared with the rows it replaces when it arrives, until one differs: from there
	 * on the new rows are taken as they come, and before it the rows shown are kept. If none
	 * differs and the count is the same, the array stays as it is.
	 */
	async #reload() {
		while (this.#busy) await this.#busy;
		await this.#exclusive(async () => {
			const shown = this.#items;
			const rows: T[] = [];
			let differs: number | undefined;
			let page: Page<T>;
			let cursor: string | undefined;
			do {
				// No more rows than are shown, so an unchanged window reads back the same rows; an
				// empty list reads a first page of the usual size.
				const wanted = shown.length - rows.length;
				page = await this.load(cursor, wanted > 0 ? Math.min(RELOAD_PAGE, wanted) : undefined);
				if (
					differs === undefined &&
					!same(page.items, shown.slice(rows.length, rows.length + page.items.length))
				) {
					differs = rows.length;
				}
				rows.push(...page.items);
				cursor = page.nextCursor ?? undefined;
			} while (cursor && rows.length < shown.length);
			if (differs !== undefined || rows.length !== shown.length) {
				const kept = differs ?? Math.min(rows.length, shown.length);
				this.#items = [...shown.slice(0, kept), ...rows.slice(kept)];
			}
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
		return () => {
			// A pump still running for this run must stop, or nothing would ever end it.
			visible = false;
			observer.disconnect();
		};
	};
}
