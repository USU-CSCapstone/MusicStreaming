// Reporting this player's progress to the server, which keeps the account's playback state and
// tells every device (`api/openapi.yaml`, `ProgressReport`): exactly one device plays at a
// time, and starting playback anywhere takes over (`requirements/realtime.md` §2).
//
// A report that takes over carries `start`. It is sent when the user starts playback here,
// and on a new connection, which is never the active one. Any other report is sent only while
// this device is playing as far as the server knows, from a takeover sent (the server takes a
// connection's reports in turn) or the latest state; from elsewhere it would be refused. A
// state naming another device means this player was taken over from and stops, unless it comes
// before the server has confirmed a takeover from here, when it is from before that takeover.

import type { PlayerState, ProgressReport } from './api/types';

/** Where the player is, as a report gives it. */
export type Snapshot = Omit<ProgressReport, 'start' | 'at'>;

/** What a player does about a state from the server. */
export type Action = 'stop' | 'report' | null;

/** How long a takeover may go unconfirmed before states naming another device are obeyed. */
const CONFIRM_WITHIN_MS = 5_000;

export class Progress {
	#send: (report: ProgressReport) => boolean;
	/** Whether the next report takes over. */
	#start = false;
	/** Whether that is because the user started playback here, which no older state undoes. */
	#byUser = false;
	/** When a report that took over went, until a state confirms it. */
	#startedAt: number | null = null;
	/** Whether the server has this device as playing, as far as it has said. */
	#active = false;

	/** `send` sends a report, and answers whether it went. */
	constructor(send: (report: ProgressReport) => boolean) {
		this.#send = send;
	}

	/** The user started playback here. */
	started() {
		this.#start = this.#byUser = true;
	}

	/**
	 * The user paused, or another tab took over, before a takeover from here could be sent: there
	 * is nothing to take.
	 */
	paused() {
		if (this.#byUser) this.#start = this.#byUser = false;
	}

	/** A new connection: it is never the active one, so the next report must take over. */
	connected() {
		this.#start = true;
		this.#startedAt = null;
		this.#active = false;
	}

	/**
	 * Reports `now`, if the server would take it. A takeover the server refuses anyway, as for
	 * a track no longer in reach, is not tried again: the device is stopped by the first state
	 * naming another once it could have been confirmed, or plays on unreported.
	 */
	report(now: Snapshot) {
		if (!this.#start && !this.#active) return;
		// The server takes no stop as a takeover.
		const start = this.#start && now.status !== 'stopped';
		if (!this.#send({ ...now, start })) return;
		if (start) {
			this.#start = this.#byUser = false;
			this.#startedAt = Date.now();
			this.#active = true;
		}
	}

	/**
	 * What a player that is `playing` (or loading a track to play) on device `deviceId` does
	 * about `state`. A takeover still due, as on a new connection, is settled by the state: it
	 * goes ahead only if this player plays and nothing else does, unless the server has just
	 * refused it, or the user started it here.
	 */
	heard(deviceId: string, state: PlayerState, playing: boolean): Action {
		const here = state.activeDeviceId === deviceId;
		// Naming another device before this one's takeover is confirmed: from before it, unless it
		// was refused long enough ago.
		const unconfirmed = here ? null : this.#startedAt;
		if (unconfirmed !== null && Date.now() - unconfirmed < CONFIRM_WITHIN_MS) return null;
		const refused = unconfirmed !== null;
		this.#active = here;
		this.#startedAt = null;
		if (!playing) {
			this.#start = this.#byUser;
			return null;
		}
		// As after connecting while a connection of this device that is gone still plays.
		if (here) return this.#start ? 'report' : null;
		if ((state.activeDeviceId === null && !refused) || this.#byUser) {
			this.#start = true;
			return 'report';
		}
		this.#start = false;
		return state.activeDeviceId === null ? null : 'stop';
	}
}
