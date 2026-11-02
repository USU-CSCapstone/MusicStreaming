// Rows learned from the server's pages (`design/queue.md` §3): what a device holds of the rows
// whose lists have not arrived, filled by item. An item's track never changes while the item
// exists, so a row fetched at any version is right at every later one.

import type { QueueRows } from '$lib/api/types';

/** How many rows a page holds, and the most the server gives. */
export const PAGE = 100;

/** A part of the queue pages are of. */
export type Section = 'upNext' | 'history';

/** Fetches pages of rows on demand and holds each item's track, so the page can show rows before their lists arrive. */
export class RowPages {
	/** Each fetched item's track, or null where the server could not read it. */
	#items = new Map<string, string | null>();
	/** The version each page was last asked at; a page is asked again only when that changes. */
	#asked = new Map<string, number>();
	/** How many fetches of each page, as `section:page`, are under way. */
	#fetching = new Map<string, number>();
	#queueId: string | null = null;
	/** Counts queues, so a page that comes back after another queue replaced this one is dropped. */
	#generation = 0;
	#get: (path: string) => Promise<Response | null>;
	#settled: () => void;

	/** Makes pages fetched with `get`, which answers a path under the API or null; `settled` is called after each. */
	constructor(get: (path: string) => Promise<Response | null>, settled: () => void) {
		this.#get = get;
		this.#settled = settled;
	}

	/** The track of `item` from a page: a string, null if the server could not read it, or undefined if not held. */
	track(item: string): string | null | undefined {
		return this.#items.get(item);
	}

	/** Whether the page holding row `index` of `section` is being fetched. */
	fetching(section: Section, index: number): boolean {
		return this.#fetching.has(`${section}:${Math.floor(index / PAGE)}`);
	}

	/** Drops what pages gave for the items of the sources `keys`, once their lists are attached. */
	forgetSources(keys: Set<string>) {
		for (const item of this.#items.keys()) {
			if (keys.has(item.slice(0, item.lastIndexOf('.')))) this.#items.delete(item);
		}
	}

	/** Drops one item, whose list has arrived. */
	forget(item: string) {
		this.#items.delete(item);
	}

	/** Starts afresh for another queue. */
	reset(queueId: string | null) {
		if (queueId === this.#queueId) return;
		this.#queueId = queueId;
		this.#generation++;
		this.#items.clear();
		this.#asked.clear();
		this.#fetching.clear();
	}

	/** Lets every page be asked for again, as when the connection is back. */
	retry() {
		this.#asked.clear();
	}

	/**
	 * Fetches, in the background, each page that holds a row of `section` from `offset` for
	 * `limit` rows, unless it was asked for already at this `version`. A page that fails is
	 * therefore not tried again until the version changes or `retry` is called.
	 */
	ensure(queueId: string, version: number, section: Section, offset: number, limit: number) {
		this.reset(queueId);
		const last = Math.floor((offset + Math.max(limit, 1) - 1) / PAGE);
		for (let page = Math.floor(offset / PAGE); page <= last; page++) {
			const key = `${section}:${page}`;
			if (this.#asked.get(key) === version) continue;
			this.#asked.set(key, version);
			void this.#fetch(section, page);
		}
	}

	/** Fetches page number `page` and keeps its rows by item. */
	async #fetch(section: Section, page: number) {
		const generation = this.#generation;
		const queueId = this.#queueId;
		const key = `${section}:${page}`;
		this.#fetching.set(key, (this.#fetching.get(key) ?? 0) + 1);
		try {
			const response = await this.#get(
				`/me/queue/rows?section=${section}&offset=${page * PAGE}&limit=${PAGE}`
			);
			if (!response?.ok) return;
			const rows = (await response.json()) as QueueRows;
			if (generation !== this.#generation || rows.queueId !== queueId) return;
			for (const row of [...(rows.current ? [rows.current] : []), ...rows.rows]) {
				this.#items.set(row.item, row.trackId);
			}
		} catch {
			// A page that cannot be read is treated as one that failed.
		} finally {
			if (generation === this.#generation) {
				const left = (this.#fetching.get(key) ?? 1) - 1;
				if (left > 0) this.#fetching.set(key, left);
				else this.#fetching.delete(key);
				this.#settled();
			}
		}
	}
}
