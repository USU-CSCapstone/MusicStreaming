// Keeps the installed app current without asking: the browser is asked to look for a new service
// worker (`src/service-worker.ts`), which takes over by itself, so the next load runs the new build.

/**
 * Asks the browser to check for a new version of the app, in the background. The one place a
 * check starts, so a gate such as Work offline wraps it here; does nothing without a worker.
 */
export function checkForUpdate() {
	if (!('serviceWorker' in navigator)) return;
	navigator.serviceWorker
		.getRegistration()
		.then((registration) => registration?.update())
		.catch(() => {
			// No network, or no worker yet: the next check tries again.
		});
}
