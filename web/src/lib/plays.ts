// Recording plays (`requirements/analytics.md` §1): every track that starts playing is one,
// and its listen time is the audio heard. Pausing adds nothing, nor does skipping forward;
// replaying part of it adds that part again. A play is reported when it starts, when it
// pauses, and when it ends, each time in full under its own ID, so the server can take them
// in any order and count it once.

import type { PlayContext, PlayReport } from './api/types';

export type End = NonNullable<PlayReport['end']>;

/** Sends reports; `keepalive` when the page is closing and the request must outlive it. */
export type Send = (plays: PlayReport[], keepalive: boolean) => Promise<void>;

/** The most media time one `timeupdate` can add; more than that is a jump, not listening. */
const MAX_STEP = 1;

export class PlayRecorder {
	#send: Send;
	#play: PlayReport | null = null;
	/** Seconds heard so far, kept unrounded. */
	#heard = 0;
	/** The media time last heard, from which the next is counted. */
	#last: number | null = null;

	constructor(send: Send) {
		this.#send = send;
	}

	/** A track has started playing at media time `time`. Ends any play before it as skipped. */
	start(libraryId: string, trackId: string, context: PlayContext, time = 0) {
		this.end('skipped');
		this.#play = {
			playId: crypto.randomUUID(),
			libraryId,
			trackId,
			startedAt: new Date().toISOString(),
			listenTimeMs: 0,
			end: null,
			context,
			origin: 'context'
		};
		this.#heard = 0;
		this.#last = time;
		this.#report(false);
	}

	/** The audio has reached media time `time`, playing from where it was last heard. */
	heard(time: number) {
		if (!this.#play) return;
		const step = this.#last === null ? 0 : time - this.#last;
		if (step > 0 && step <= MAX_STEP) this.#heard += step;
		this.#last = time;
	}

	/** It moved to `time` without playing what lies between. */
	seeked(time: number) {
		this.#last = time;
	}

	paused() {
		if (this.#play) this.#report(false);
	}

	/** The play is over, `end` saying how. Nothing happens if none is under way. */
	end(end: End, keepalive = false) {
		if (!this.#play) return;
		this.#play.end = end;
		this.#report(keepalive);
		this.#play = null;
	}

	#report(keepalive: boolean) {
		const play = { ...this.#play!, listenTimeMs: Math.round(this.#heard * 1000) };
		// A lost report is resent with the next one for the same play, or lost with the page.
		this.#send([play], keepalive).catch(() => {});
	}
}
