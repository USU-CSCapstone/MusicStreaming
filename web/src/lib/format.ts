// Display formatting shared across views.

/** `m:ss`, or `h:mm:ss` from an hour up, from integer microseconds. */
export function formatDuration(us: number): string {
	const total = Math.max(0, Math.floor(us / 1_000_000));
	const h = Math.floor(total / 3600);
	const m = Math.floor((total % 3600) / 60);
	const s = total % 60;
	const ss = String(s).padStart(2, '0');
	return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

/** A collection's length in words: "42 min" or "3 hr 5 min". */
export function formatLength(us: number): string {
	const minutes = Math.round(us / 60_000_000);
	if (minutes < 60) return `${minutes} min`;
	const h = Math.floor(minutes / 60);
	const m = minutes % 60;
	return m === 0 ? `${h} hr` : `${h} hr ${m} min`;
}

/** Untagged albums and artists are real entities with a `null` name (`requirements/scanning.md` §2). */
export function albumTitle(title: string | null): string {
	return title ?? 'Unknown Album';
}

export function artistName(name: string | null): string {
	return name ?? 'Unknown Artist';
}

/** The year from a partial release date (`YYYY`, `YYYY-MM`, or `YYYY-MM-DD`). */
export function releaseYear(date: string | null | undefined): string | null {
	return date ? date.slice(0, 4) : null;
}

export function plural(n: number, one: string, many = `${one}s`): string {
	return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}
