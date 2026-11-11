// Recording plays (`requirements/analytics.md` §1): every track that starts playing is one,
// and its listen time is the audio heard. Pausing adds nothing, nor does skipping forward;
// replaying part of it adds that part again. A play is reported when it starts, when it
// pauses, when it ends, and as a checkpoint while it goes on, each time in full under its own
// ID, so the server can take them in any order and count it once. The log keeps what is
// reported until the server has it (`sync/sender.ts`).

import type { PlayContext, PlayReport } from './api/types';

export type End = NonNullable<PlayReport['end']>;

/** Takes reports; `keepalive` when the page is closing and the request must outlive it. */
export type Send = (plays: PlayReport[], keepalive: boolean) => void;

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

	/** Whether a play is under way: started, and not yet ended. */
	get active() {
		return this.#play !== null;
	}

	/**
	 * A track has started playing at media time `time`, from the context or the Manual Queue as
	 * `origin` says. Ends any play before it as skipped.
	 */
	start(
		libraryId: string,
		trackId: string,
		context: PlayContext,
		origin: PlayReport['origin'],
		time = 0
	) {
		this.end('skipped');
		this.#play = {
			playId: crypto.randomUUID(),
			libraryId,
			trackId,
			startedAt: new Date().toISOString(),
			listenTimeMs: 0,
			end: null,
			context,
			origin
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

	/**
	 * Reports the play under way as it stands, so a page closed without warning loses none of the
	 * time heard.
	 */
	checkpoint() {
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
		this.#send([play], keepalive);
	}
}
