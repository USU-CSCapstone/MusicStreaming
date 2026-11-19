/** One page of a cursor-paged list: its rows, and where the next page starts, if there is one. */
type AdminPage<T> = { items: T[]; nextCursor: string | null };

/**
 * A list an admin page shows, read page by page from the server: the rows loaded so far, whether
 * more follow, and the means to read the next page, start over, or replace a row with the
 * server's newer answer.
 */
export class AdminList<T> {
	#items = $state.raw<T[]>([]);
	#cursor = $state<string | null>(null);
	/** The read under way; another ask for the same page waits for it rather than running beside. */
	#busy: Promise<void> | null = null;

	/** `first` is the first page, already read; `load` reads the page after `cursor`, or the first. */
	constructor(
		first: AdminPage<T>,
		private load: (cursor?: string) => Promise<AdminPage<T>>
	) {
		this.#items = first.items;
		this.#cursor = first.nextCursor;
	}

	get items(): T[] {
		return this.#items;
	}

	get hasMore(): boolean {
		return this.#cursor !== null;
	}

	/** Reads the next page and adds it; rejects if the read fails, leaving the list as it was. */
	loadMore(): Promise<void> {
		const cursor = this.#cursor;
		if (cursor === null) return Promise.resolve();
		return (this.#busy ??= this.load(cursor)
			.then((page) => {
				this.#items = [...this.#items, ...page.items];
				this.#cursor = page.nextCursor;
			})
			.finally(() => (this.#busy = null)));
	}

	/** Reads the first page again, and drops the rest. */
	async reload(): Promise<void> {
		const page = await this.load();
		this.#items = page.items;
		this.#cursor = page.nextCursor;
	}

	/** Puts `item` where the row `is` true of is, as the server now has it. */
	replace(item: T, is: (row: T) => boolean): void {
		this.#items = this.#items.map((row) => (is(row) ? item : row));
	}
}
