// Where a catalog read gets its answer from (`design/general.md` §7.1): the server or the
// device's copy of the library. `api/client.ts` calls `read` for every read the device can
// answer, so pages never know which. Page cursors need no source: server and device build and
// read them with the same crate, so either accepts the other's.
//
// - Library not fully on the device: the server. When the device knows it is offline, or the
//   server cannot be reached while the realtime connection is down, an error saying so, never
//   part of it.
// - On the device: the device alone, at once, even while a pull is under way.
// - A device that cannot answer: the server, as if the device were not there; offline, an
//   error saying the page is unavailable.
//
// Search is the other way round (`requirements/search.md` §2): the server whenever it is
// reachable, the device when offline or when the server fails (`readSearch`).

import { ApiError } from '$lib/api/root';
import { realtime } from '$lib/realtime.svelte';
import { catalog } from './client.svelte';

/** What a page that needs the server says when this device cannot reach it. */
const UNAVAILABLE = 'This needs the server, which this device cannot reach right now.';

/**
 * Whether this device cannot reach the server: the browser says so, or the realtime connection
 * has been lost.
 */
export const offline = () => navigator.onLine === false || realtime.offline;

/** Whether the device itself knows it cannot reach the server, so asking would only wait. */
export const knownOffline = () => navigator.onLine === false;

/**
 * For a failed request to the server: says it is unavailable offline when the device is, and
 * otherwise rethrows.
 */
export function unavailableIfOffline(error: unknown): never {
	if (offline()) throw new ApiError(503, 'unavailable_offline', UNAVAILABLE);
	throw error;
}

/**
 * What a read says of a library that is not on the device: not downloaded yet, or, where an
 * owner of another version of the app holds the device, that the app needs updating to read it.
 */
const notOnDevice = () =>
	catalog.outdated
		? new ApiError(503, 'not_on_device', 'This app needs updating to read this device’s music.')
		: new ApiError(
				503,
				'not_on_device',
				'This library has not been downloaded to this device yet.'
			);

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
	if (!catalog.onDevice(library)) {
		if (knownOffline()) throw notOnDevice();
		// A realtime connection in backoff says nothing about HTTP: ask, and call a request that
		// never reached the server offline; an answer, such as a redirect to log in, stands.
		return server().catch((error) => {
			throw offline() && unreachable(error) ? notOnDevice() : error;
		});
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

/**
 * Whether a failed request to the server says it cannot be reached or is broken, not that the
 * request is wrong.
 */
export const unreachable = (error: unknown) =>
	error instanceof TypeError || (error instanceof ApiError && error.status >= 500);

/**
 * The answer to the API operation `name` of `library` with `params`, for a read the server
 * answers whenever it can be reached: the device answers when this one is offline or the server
 * fails, and only if the library is on it. A refusal by the server (a request it finds invalid)
 * is never the device's to second-guess.
 */
export async function readSearch<T>(
	name: string,
	library: string,
	params: object,
	server: () => Promise<T>
): Promise<T> {
	let failure: unknown;
	if (!offline()) {
		try {
			return await server();
		} catch (error) {
			if (!unreachable(error)) throw error;
			failure = error;
		}
	}
	if (!catalog.onDevice(library)) {
		if (failure) throw failure;
		if (knownOffline()) throw notOnDevice();
		// Skipped while the realtime connection is down, which says nothing about HTTP: with no
		// device copy to answer instead, the server is worth asking.
		return server().catch((error) => {
			throw unreachable(error) ? notOnDevice() : error;
		});
	}
	try {
		// Reached only with the server offline, failing or slow, so the device waits for an
		// update under way, or answers from a copy that no longer updates, as it does offline.
		return JSON.parse(await catalog.query(library, name, params, true)) as T;
	} catch (error) {
		if (failure) throw failure;
		console.error(error);
		throw new ApiError(503, 'unavailable_offline', UNAVAILABLE);
	}
}
