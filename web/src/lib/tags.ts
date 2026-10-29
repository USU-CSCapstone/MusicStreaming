// How tag types are named to people (`requirements/tags.md` §2).

import type { TagType } from '$lib/api/types';

/** The built-in types' names, for one and for more than one. */
const NAMES: Record<string, [string, string]> = {
	genre: ['Genre', 'Genres'],
	mood: ['Mood', 'Moods']
};

/**
 * The name of the tag type `key`, such as `Genre`: a built-in's own, the name the plugin that
 * added it gave it in `types` (`listTagTypes`), or else its key, capitalized.
 */
export function tagTypeName(key: string, types: readonly TagType[] = []): string {
	return (
		NAMES[key]?.[0] ??
		types.find((type) => type.key === key)?.name ??
		key.charAt(0).toUpperCase() + key.slice(1)
	);
}

/**
 * The name of the tag type `key` for more than one, such as `Genres`. A plugin gives its type
 * one name, which serves for both.
 */
export function tagTypePlural(key: string, types: readonly TagType[] = []): string {
	return NAMES[key]?.[1] ?? tagTypeName(key, types);
}
