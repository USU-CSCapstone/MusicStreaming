// Recording recent searches (`requirements/search.md` §6). Results update on every keystroke,
// so a search is recorded only once it settles: when the user acts on one of its results,
// submits it, or leaves it after it has stood for `STAND_MS`. A search that found nothing still
// settles, so it can be offered again. Each query is recorded at most once while it is on
// screen, except to add the result acted on.

/** How long a query must stand before leaving it records it. */
export const STAND_MS = 2000;

/** A result the user acted on. Search returns tracks, albums, and artists. */
export type Selected = { type: 'track' | 'album' | 'artist'; id: string };

/** Records a search; `keepalive` when the page is closing and the request must outlive it. */
export type Record = (
	libraryId: string,
	query: string,
	selected: Selected | undefined,
	keepalive: boolean
) => void;

export class SearchSettler {
	#record: Record;
	#now: () => number;
	#libraryId = '';
	#query = '';
	#since = 0;
	#recorded = false;

	constructor(record: Record, now = () => Date.now()) {
		this.#record = record;
		this.#now = now;
	}

	/** `query` is on screen in library `libraryId`, as the user types it. */
	shown(libraryId: string, query: string) {
		const q = query.trim();
		if (libraryId === this.#libraryId && q === this.#query) return;
		this.#libraryId = libraryId;
		this.#query = q;
		this.#since = this.#now();
		this.#recorded = false;
	}

	/** The user submitted the search, or acted on `selected` among its results. */
	settle(selected?: Selected) {
		if (!this.#query || (this.#recorded && !selected)) return;
		this.#recorded = true;
		this.#record(this.#libraryId, this.#query, selected, false);
	}

	/** The user is leaving the search, `closing` the page or not. */
	leave(closing = false) {
		const stood = this.#now() - this.#since >= STAND_MS;
		if (this.#query && !this.#recorded && stood) {
			this.#record(this.#libraryId, this.#query, undefined, closing);
		}
		// Coming back to the same words later is a new search.
		this.#query = '';
		this.#recorded = false;
	}
}
