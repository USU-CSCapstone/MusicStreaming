// What the panel's kept queues list says about a kept queue (`requirements/queue.md` §6): the
// context's name, and when it was last active.

import type { PlayContext } from '$lib/api/types';

/**
 * What names a context, or null for one whose name needs no lookup: an album, artist or
 * playlist is named by its `id` in a library.
 */
export function namedBy(
	context: PlayContext
): { kind: 'album' | 'artist' | 'playlist'; id: string } | null {
	return context.type === 'album' || context.type === 'artist' || context.type === 'playlist'
		? { kind: context.type, id: context.id }
		: null;
}

/** What a queue started from, in words: `name` is the album's, artist's or playlist's, if known. */
export function contextLabel(context: PlayContext, name?: string): string {
	switch (context.type) {
		case 'album':
			return name ?? 'An album';
		case 'artist':
			return name ?? 'An artist';
		case 'playlist':
			return name ?? 'A playlist';
		case 'search':
			return `Search “${context.query}”`;
		case 'track':
			return 'A song';
		case 'tag':
			return 'A tag';
		case 'mix':
			return 'A mix';
		case 'radio':
			return 'Radio';
		case 'external':
			return 'Another service';
		default:
			return 'Your library';
	}
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
	['day', 86_400_000],
	['hour', 3_600_000],
	['minute', 60_000]
];
const relative = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });

/** How long before `now` the ISO time `iso` was, such as "3 days ago", or "just now" inside a minute. */
export function ago(iso: string, now = Date.now()): string {
	const elapsed = now - new Date(iso).getTime();
	for (const [unit, ms] of UNITS) {
		if (elapsed >= ms) return relative.format(-Math.floor(elapsed / ms), unit);
	}
	return 'just now';
}
