// A clock for tests of code that takes a `Clock`: time passes only when a test says so.

import type { Clock } from './router';

/** Timers that fire, earliest first, only as `advance` passes them. */
export class FakeClock implements Clock {
	#now = 0;
	#timers = new Set<{ at: number; fn: () => void }>();

	/** Starts a timer for `ms` from now; the returned function cancels it. */
	after(ms: number, fn: () => void) {
		const timer = { at: this.#now + ms, fn };
		this.#timers.add(timer);
		return () => void this.#timers.delete(timer);
	}

	/** How many timers are waiting. */
	get waiting() {
		return this.#timers.size;
	}

	/**
	 * Passes `ms` of time, firing each timer that falls due on the way, earliest first, and yielding
	 * a task after each so what it starts, and the messages and locks that follow, can run.
	 */
	async advance(ms: number) {
		const end = this.#now + ms;
		for (;;) {
			const due = [...this.#timers].filter((timer) => timer.at <= end);
			const next = due.sort((a, b) => a.at - b.at)[0];
			if (!next) break;
			this.#timers.delete(next);
			this.#now = next.at;
			next.fn();
			await new Promise((resolve) => setTimeout(resolve));
		}
		this.#now = end;
		await new Promise((resolve) => setTimeout(resolve));
	}
}
