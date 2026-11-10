// The tag filter's rules, in no framework's terms, so the app and its tests match tags
// alike (`requirements/tags.md` §2, §7).

import type { TagsByType } from './api/types';

/** Whether an item must carry all or any of a type's chosen tags. */
export type TagMatch = 'any' | 'all';

/**
 * The tags a list is narrowed to, by ID, each list in the order its tags were chosen, as the
 * API's `tagAll` and `tagAny` name them: an item carries every tag of `all` and, since types are
 * independent (`requirements/tags.md` §2), one of `any`'s of each type.
 */
export type TagFilter = { all: string[]; any: string[] };

/** No filter. */
export const UNFILTERED: TagFilter = { all: [], any: [] };

/** Whether `filter` narrows a list at all. */
export function isFiltered(filter: TagFilter): boolean {
	return filter.all.length > 0 || filter.any.length > 0;
}

/** Every tag `filter` names, `all`'s first. */
export function chosenTags(filter: TagFilter): string[] {
	return [...filter.all, ...filter.any];
}

/**
 * The items of `items` whose tags `filter` keeps, in their order, as the server keeps them: every
 * tag of `filter.all`, and one of `filter.any`'s of each type. An `any` tag's type is read off the
 * items that carry it, or else from `typeOf`. One whose type is not known is a group of its own
 * that nothing matches, as a tag the library does not have matches nothing.
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
	for (const tag of filter.any) {
		const type = carried.get(tag) ?? typeOf(tag) ?? `unknown:${tag}`;
		groups.set(type, [...(groups.get(type) ?? []), tag]);
	}
	return items.filter((item) => {
		const has = new Set(Object.values(tagsOf(item)).flatMap((tags) => tags.map((tag) => tag.id)));
		return (
			filter.all.every((tag) => has.has(tag)) &&
			[...groups.values()].every((group) => group.some((tag) => has.has(tag)))
		);
	});
}

/**
 * Whether `filterByTags` can tell every `filter.any` tag's type: `known` says its type has been
 * read, or one of `items` carries it. Until then it may leave out items the server keeps, which
 * groups a tag it knows nothing of by itself. `items` are read only for tags still unknown.
 */
export function tagTypesKnown<T>(
	items: readonly T[],
	filter: TagFilter,
	tagsOf: (item: T) => TagsByType,
	known: (tag: string) => boolean
): boolean {
	const unknown = filter.any.filter((tag) => !known(tag));
	if (unknown.length === 0) return true;
	const carried = new Set(
		items.flatMap((item) => Object.values(tagsOf(item)).flatMap((tags) => tags.map((t) => t.id)))
	);
	return unknown.every((tag) => carried.has(tag));
}
