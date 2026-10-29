// What is known of tags by ID, each read once and shared: the filter bar names the tags chosen,
// and a list that filters on the device groups them by type as the server does
// (`requirements/tags.md` §2).

import { SvelteMap } from 'svelte/reactivity';
import { ApiError, getTag } from './api/client';
import type { Tag } from './api/types';

/** A tag's name and type. */
export type TagInfo = Pick<Tag, 'name' | 'type'>;

class KnownTags {
	/** By `{library}/{id}`: what is known, or `null` for a tag the library does not have. */
	#tags = new SvelteMap<string, TagInfo | null>();
	/** The tags read or being read, so each is read once. */
	#asked = new Set<string>();

	/** Tag `id` of `library`: its name and type, `null` if the library has none, `undefined` until read. */
	get(library: string, id: string): TagInfo | null | undefined {
		return this.#tags.get(`${library}/${id}`);
	}

	/**
	 * Reads each of `ids` not read yet. It changes state as they arrive, so call it from an effect,
	 * never while deriving. A failure other than the tag being gone is tried again next time.
	 */
	load(library: string, ids: readonly string[]) {
		for (const id of ids) {
			const key = `${library}/${id}`;
			if (this.#asked.has(key)) continue;
			this.#asked.add(key);
			getTag(fetch, library, id)
				.then((tag) => this.#tags.set(key, { name: tag.name, type: tag.type }))
				.catch((error) => {
					if (error instanceof ApiError && error.status === 404) this.#tags.set(key, null);
					else this.#asked.delete(key);
				});
		}
	}

	/** Records a tag already in hand, such as one picked from a list of them. */
	note(library: string, tag: Pick<Tag, 'id' | 'name' | 'type'>) {
		const key = `${library}/${tag.id}`;
		this.#asked.add(key);
		this.#tags.set(key, { name: tag.name, type: tag.type });
	}
}

export const knownTags = new KnownTags();
