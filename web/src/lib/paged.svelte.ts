import type { Page } from './api/types';

/** A cursor-paged list that grows as the user scrolls. */
export class Paged<T> {
	items = $state<T[]>([]);
	total = $state(0);
	#cursor: string | null;
	#loading = false;

	constructor(
		first: Page<T>,
		private load: (cursor: string) => Promise<Page<T>>
	) {
		this.items = first.items;
		this.total = first.total;
		this.#cursor = first.nextCursor;
	}

	get hasMore(): boolean {
		return this.#cursor !== null;
	}

	async more() {
		if (this.#loading || this.#cursor === null) return;
		this.#loading = true;
		try {
			const next = await this.load(this.#cursor);
			this.items.push(...next.items);
			this.total = next.total;
			this.#cursor = next.nextCursor;
		} finally {
			this.#loading = false;
		}
	}

	/**
	 * An attachment for an element at the end of the list: loads pages while it is near
	 * the viewport, so a page too short to fill the screen still leads to the next.
	 */
	sentinel = (el: Element) => {
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
			{ rootMargin: '800px' }
		);
		observer.observe(el);
		return () => observer.disconnect();
	};
}
