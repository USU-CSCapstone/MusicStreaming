// Where this device stands with the server (`requirements/offline.md` §7), the one answer to
// where a read goes:
//
// - `online`: the server answers.
// - `unreachable`: the realtime connection was lost or never came up, or a request failed for
//   want of an answer. HTTP is still tried, and the device's copy answers when it fails.
// - `offline`: the browser says it has no network. No request is made.
// - `working-offline`: the user chose it, to protect a metered connection. No request is made,
//   the realtime connection included; the choice is kept in this browser and shared by its tabs.
//
// Any realtime message, or an HTTP answer below 500, returns to `online`. The workers are told
// only whether to make requests (Work offline); they follow the browser's own network state
// themselves.

import { online } from 'svelte/reactivity/window';

/** The `localStorage` key that keeps the Work offline choice. */
const KEY = 'jewelcase-work-offline';

/** Whether the browser kept Work offline on; off where it keeps nothing. */
function kept(): boolean {
	try {
		return localStorage.getItem(KEY) === '1';
	} catch {
		return false;
	}
}

/** Where this device stands with the server. */
export type State = 'online' | 'unreachable' | 'offline' | 'working-offline';

/** The device's connectivity choice; the app uses the one instance, `connectivity`. */
export class Connectivity {
	/** Whether the user has this device working offline. */
	workOffline = $state(kept());
	/** Whether the server was last found not to answer; cleared by any answer. */
	#unreachable = $state(false);
	/** Who hears that Work offline was turned off. */
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- listeners, which nothing shows
	#resumed = new Set<() => void>();

	/** Starts following the choice other tabs of this browser make. */
	constructor() {
		globalThis.addEventListener?.('storage', (event) => {
			if (event.key === KEY) this.#set(kept());
		});
	}

	/**
	 * Calls `listener` each time Work offline is turned off, in this tab or another; returns how to
	 * stop.
	 */
	onResume(listener: () => void): () => void {
		this.#resumed.add(listener);
		return () => this.#resumed.delete(listener);
	}

	/** Sets the choice, telling who listens if it was turned off. */
	#set(on: boolean) {
		const was = this.workOffline;
		this.workOffline = on;
		if (was && !on) for (const listener of this.#resumed) listener();
	}

	/** Where the device stands with the server. */
	get state(): State {
		if (this.workOffline) return 'working-offline';
		if (online.current === false) return 'offline';
		return this.#unreachable ? 'unreachable' : 'online';
	}

	/** Whether no request is worth making: working offline, or the browser has no network. */
	get blocked() {
		const state = this.state;
		return state === 'offline' || state === 'working-offline';
	}

	/**
	 * Records that the server did not answer: the realtime connection stayed down, or a request
	 * failed for reachability.
	 */
	lose() {
		this.#unreachable = true;
	}

	/** Records that the server answered, or that there is no connection left to find lost. */
	reach() {
		this.#unreachable = false;
	}

	/**
	 * Turns Work offline on or off, and keeps the choice for this browser; storage that refuses it
	 * keeps it for this tab only.
	 */
	setWorkOffline(on: boolean) {
		this.#set(on);
		try {
			if (on) localStorage.setItem(KEY, '1');
			else localStorage.removeItem(KEY);
		} catch {
			// Private windows and full disks: the choice then lasts until the page closes.
		}
	}

	/** What the connectivity indicator says; nothing while online. */
	get label() {
		const state = this.state;
		return state === 'online' ? '' : state === 'working-offline' ? 'Working offline' : 'Offline';
	}
}

/** The one instance the app and its tabs' pages read. */
export const connectivity = new Connectivity();
