// Where a catalog read gets its answer from (`design/general.md` §7.1): the server or the
// device's copy of the library. `api/client.ts` calls `read` for every read the device can
// answer, so pages never know which. Page cursors need no source: server and device build and
// read them with the same crate, so either accepts the other's.
//
// - Library not fully on the device: the server. Offline, an error saying so, never part of it.
// - On the device: the device alone, at once, even while a pull is under way.
// - A device that cannot answer: the server, as if the device were not there; offline, an
//   error saying the page is unavailable.

import { ApiError } from '$lib/api/root';
import { realtime } from '$lib/realtime.svelte';
import { catalog } from './client.svelte';

/** What a page that needs the server says when this device cannot reach it. */
const UNAVAILABLE = 'This needs the server, which this device cannot reach right now.';

/** Whether this device cannot reach the server: the browser says so, or the realtime connection has been lost. */
export const offline = () => navigator.onLine === false || realtime.offline;

/** For a failed request to the server: says it is unavailable offline when the device is, and otherwise rethrows. */
export function unavailableIfOffline(error: unknown): never {
	if (offline()) throw new ApiError(503, 'unavailable_offline', UNAVAILABLE);
	throw error;
}

/**
 * The answer to the API operation `name` of `library` with `params`, spelled as the API spells
 * them. `server` asks the server the same thing. Rejects with an `ApiError` when the answer
 * needs the server and the device is offline.
 */
export async function read<T>(
	name: string,
	library: string,
	params: object,
	server: () => Promise<T>
): Promise<T> {
	if (!(await catalog.onDevice(library))) {
		if (offline()) {
			throw new ApiError(
				503,
				'not_on_device',
				'This library has not been downloaded to this device yet.'
			);
		}
		return server();
	}
	let local: unknown;
	try {
		local = JSON.parse(await catalog.query(library, name, params, offline()));
	} catch (error) {
		if (offline()) throw new ApiError(503, 'unavailable_offline', UNAVAILABLE);
		console.error(error);
		return server();
	}
	// A get that finds nothing is the server's to confirm.
	if (local === null) {
		if (offline()) throw new ApiError(404, 'not_found', 'Not found.');
		return server();
	}
	return local as T;
}
