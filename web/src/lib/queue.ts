// Moving through a play context. Tracks marked missing are skipped in place,
// leaving order and position untouched (`requirements/conventions.md` §6).

type Playable = { availability: 'available' | 'missing' };

/** Previous restarts the current track once it has played this long. */
export const RESTART_AFTER_S = 3;

/** The next playable index from `from` in direction `step`, or null at the end. */
export function nextPlayable(tracks: Playable[], from: number, step: 1 | -1 = 1): number | null {
	for (let i = from + step; i >= 0 && i < tracks.length; i += step) {
		if (tracks[i].availability === 'available') return i;
	}
	return null;
}

/** `start` itself if it can play, otherwise the next one after it that can. */
export function firstPlayable(tracks: Playable[], start: number): number | null {
	if (start >= 0 && start < tracks.length && tracks[start].availability === 'available')
		return start;
	return nextPlayable(tracks, start, 1);
}

/** What Previous does: restart the track when it is under way or first, else step back. */
export function previousTarget(
	tracks: Playable[],
	index: number,
	positionS: number
): { restart: true } | { restart: false; index: number } {
	if (positionS > RESTART_AFTER_S) return { restart: true };
	const prev = nextPlayable(tracks, index, -1);
	return prev === null ? { restart: true } : { restart: false, index: prev };
}
