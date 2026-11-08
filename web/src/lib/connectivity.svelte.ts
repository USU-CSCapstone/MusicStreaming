// Where this device stands with the server (`requirements/offline.md` §7): online, offline, or
// working offline, which the user chooses to protect a metered connection. Working offline stops
// every request the app makes, the realtime connection included, and reads the device's copy
// of the library; the state is the device's, so it is kept in this browser and shared by its tabs.

import { realtime } from './realtime.svelte';

const KEY = 'jewelcase-work-offline';

/** Whether the browser kept Work offline on; off where it keeps nothing. */
function kept(): boolean {
	try {
		return localStorage.getItem(KEY) === '1';
	} catch {
		return false;
	}
}

/** The device's connectivity choice; the app uses the one instance, `connectivity`. */
export class Connectivity {
	/** Whether the user has this device working offline. */
	workOffline = $state(kept());

	/** Starts following the choice other tabs of this browser make. */
	constructor() {
		// Another tab of this browser turned it on or off.
		globalThis.addEventListener?.('storage', (event) => {
			if (event.key === KEY) this.workOffline = kept();
		});
	}

	/** Turns Work offline on or off, and keeps the choice for this browser; storage that refuses it keeps it for this tab only. */
	setWorkOffline(on: boolean) {
		this.workOffline = on;
		try {
			if (on) localStorage.setItem(KEY, '1');
			else localStorage.removeItem(KEY);
		} catch {
			// Private windows and full disks: the choice then lasts until the page closes.
		}
	}

	/** Whether what the server last said is no longer live: working offline, or the connection lost. */
	get stale() {
		return this.workOffline || realtime.offline;
	}

	/** What the connectivity indicator says; nothing while online. */
	get label() {
		return this.workOffline ? 'Working offline' : realtime.offline ? 'Offline' : '';
	}
}

export const connectivity = new Connectivity();
