// Keeps the installed app current without asking: the browser is asked to look for a new service
// worker (`src/service-worker.ts`), which takes over by itself, so the next load runs the new
// build.

import { connectivity } from '$lib/connectivity.svelte';

/**
 * Asks the browser to check for a new version of the app, in the background. The one place a
 * check starts; does nothing without a worker, or while working offline.
 */
export function checkForUpdate() {
	if (!('serviceWorker' in navigator) || connectivity.workOffline) return;
	navigator.serviceWorker
		.getRegistration()
		.then((registration) => registration?.update())
		.catch(() => {
			// No network, or no worker yet: the next check tries again.
		});
}
