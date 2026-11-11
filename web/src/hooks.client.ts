import type { HandleClientError } from '@sveltejs/kit';
import { dev } from '$app/environment';
import { base } from '$app/paths';
import { checkForUpdate } from '$lib/app-update';
import { ApiError } from '$lib/api/root';
import { catalog } from '$lib/catalog/client.svelte';
import { LOGOUT_PENDING } from '$lib/last-known';
import { realtime } from '$lib/realtime.svelte';

/**
 * Codes of the errors the error page explains: a library not on the device, or a page that needs
 * the server.
 */
const SHOWN = new Set(['not_on_device', 'unavailable_offline']);

/**
 * Passes the code of an error the error page explains on to it; any other is logged and shows
 * SvelteKit's message.
 */
export const handleError: HandleClientError = ({ error, message }) => {
	if (error instanceof ApiError && error.code && SHOWN.has(error.code))
		return { message: error.message, code: error.code };
	console.error(error);
	return { message };
};

// Offline start needs the worker, which development leaves out. Its URL and scope are the app's
// own root, which is why SvelteKit's registration is turned off in `vite.config.ts`.
if (!dev && 'serviceWorker' in navigator) {
	navigator.serviceWorker.register(`${base}/service-worker.js`).then(checkForUpdate, console.error);
	// A page left open picks up a new version when it is looked at again.
	document.addEventListener('visibilitychange', () => !document.hidden && checkForUpdate());
}

// The server ending the session wipes the device before the login page loads.
realtime.onSessionEnd = () => catalog.signOut();

// A logout another tab made with no server in reach leaves this tab signed in, as the session
// cookie is still good: it goes to the login page, where the pending logout is told to the server.
addEventListener('storage', (event) => {
	if (event.key === LOGOUT_PENDING && event.newValue) location.assign(`${base}/login`);
});
