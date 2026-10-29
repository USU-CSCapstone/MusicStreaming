// The tag filter's rules, in no framework's terms, so the app and its test fixtures match tags
// alike (`requirements/tags.md` §2, §7).

import type { TagsByType } from './api/types';

/** Whether an item must carry all or any of the chosen tags of each type (`api/openapi.yaml`). */
export type TagMatch = 'any' | 'all';

/**
 * The tags a list is narrowed to, by ID, in the order they were chosen. The server groups them
 * by type: within one type `match` decides, and every type named must match, since types are
 * independent (`requirements/tags.md` §2).
 */
export type TagFilter = { tags: string[]; match: TagMatch };

/** No filter. */
export const UNFILTERED: TagFilter = { tags: [], match: 'any' };

/** Whether `filter` narrows a list at all. */
export function isFiltered(filter: TagFilter): boolean {
	return filter.tags.length > 0;
}

/**
 * The items of `items` whose tags `filter` keeps, in their order, as the server keeps them: the
 * chosen tags grouped by type, each type's matched by `filter.match`, and every type's at once. A
 * chosen tag's type is read off the items that carry it, or else from `typeOf`. One whose type is
 * not known is a group of its own that nothing matches, as a tag the library does not have
 * matches nothing.
 */
export function filterByTags<T>(
	items: readonly T[],
	filter: TagFilter,
	tagsOf: (item: T) => TagsByType,
	typeOf: (tag: string) => string | undefined = () => undefined
): T[] {
	if (!isFiltered(filter)) return [...items];
	const carried = new Map<string, string>();
	for (const item of items) {
		for (const [type, tags] of Object.entries(tagsOf(item))) {
			for (const tag of tags) carried.set(tag.id, type);
		}
	}
	const groups = new Map<string, string[]>();
	for (const tag of filter.tags) {
		const type = carried.get(tag) ?? typeOf(tag) ?? `unknown:${tag}`;
		groups.set(type, [...(groups.get(type) ?? []), tag]);
	}
	return items.filter((item) => {
		const has = new Set(Object.values(tagsOf(item)).flatMap((tags) => tags.map((tag) => tag.id)));
		return [...groups.values()].every((group) =>
			filter.match === 'all'
				? group.every((tag) => has.has(tag))
				: group.some((tag) => has.has(tag))
		);
	});
}
