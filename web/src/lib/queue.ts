// Moving through what a view shows: where a queue starts, and what Previous does. Tracks marked
// missing are skipped in place, leaving order and position untouched
// (`requirements/conventions.md` §6).

type Playable = { availability: 'available' | 'missing' };

/** Previous restarts the current track once it has played this long. */
export const RESTART_AFTER_S = 3;

/** `start` itself if it can play, otherwise the next one after it that can, or null. */
export function firstPlayable(tracks: Playable[], start: number): number | null {
	for (let i = Math.max(start, 0); i < tracks.length; i++) {
		if (tracks[i].availability === 'available') return i;
	}
	return null;
}

/** Whether Previous restarts the track, since it is under way or there is nothing before it. */
export function restartsOnPrevious(positionS: number, history: number): boolean {
	return positionS > RESTART_AFTER_S || history === 0;
}
