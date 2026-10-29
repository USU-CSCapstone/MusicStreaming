// How tag types are named to people (`requirements/tags.md` §2).

/** The built-in types' names. */
const NAMES: Record<string, string> = { genre: 'Genre', mood: 'Mood' };

/** The name of the tag type `key`, such as `Genre`: a built-in's own, any other's key, capitalized. */
export function tagTypeName(key: string): string {
	return NAMES[key] ?? key.charAt(0).toUpperCase() + key.slice(1);
}
