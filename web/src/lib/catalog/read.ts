// Where a catalog read gets its answer from (`design/general.md` §7.1): the server or the
// device's copy of the library. `api/client.ts` calls `read` for every read the device can
// answer, so pages never know which. Page cursors need no source: server and device build and
// read them with the same crate, so either accepts the other's.
//
// - Library not fully on the device: the server. When the device knows it is offline, or the
//   server cannot be reached, an error saying so, never part of it.
// - On the device: the device alone, at once, even while a pull is under way.
// - A device that cannot answer: the server, as if the device were not there; offline, an
//   error saying the page is unavailable.
//
// Search is the other way round (`requirements/search.md` §2): the server whenever it is
// reachable, the device when offline or when the server fails (`readSearch`).

import { ApiError } from '$lib/api/root';
import { connectivity } from '$lib/connectivity.svelte';
import { catalog } from './client.svelte';

/** What a page that needs the server says when this device cannot reach it. */
const UNAVAILABLE = 'This needs the server, which this device cannot reach right now.';

/**
 * How long a server search may take before the device answers instead. Past about a second a
 * person types on waiting (`requirements/search.md` §7); this leaves a server on a good day
 * its answer, and a device that has to build its index the first time (under a second, a few
 * on a slow phone) still ahead of a server that is hanging.
 */
export const SLOW_MS = 1500;

/**
 * For a failed request to the server: says it is unavailable when the device is not online, and
 * otherwise rethrows.
 */
export function unavailableIfNotOnline(error: unknown): never {
	if (connectivity.state !== 'online') throw new ApiError(503, 'unavailable_offline', UNAVAILABLE);
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
 * Whether `library` is on the device: from what the owner last said while online, and, where
 * the server cannot be asked (or `wait` says the caller found it unreachable), waiting for the
 * owner to serve.
 */
const onDevice = (library: string, wait = false) =>
	wait || connectivity.state !== 'online'
		? catalog.waitForDevice(library)
		: catalog.onDevice(library);

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
	if (!(await onDevice(library))) {
		if (connectivity.blocked) throw notOnDevice();
		// A realtime connection in backoff says nothing about HTTP, and an owner not serving yet
		// is why the device was not asked: ask the server, and where it cannot be reached (which
		// the request has just recorded) wait for the device as offline does. An answer, such as a
		// redirect to log in, stands.
		try {
			return await server();
		} catch (error) {
			if (!unreachable(error)) throw error;
			if (!(await onDevice(library, true)))
				// An answered failure, such as a 500, is the server's to show while online.
				throw error instanceof TypeError || connectivity.state !== 'online' ? notOnDevice() : error;
		}
	}
	/** Whether the device is read as offline: the server is not answering, or the app is offline. */
	const off = () => connectivity.state !== 'online';
	let local: unknown;
	try {
		local = JSON.parse(await catalog.query(library, name, params, off()));
	} catch (error) {
		if (off()) throw new ApiError(503, 'unavailable_offline', UNAVAILABLE);
		console.error(error);
		return server();
	}
	// A get that finds nothing is the server's to confirm.
	if (local === null) {
		if (off()) throw new ApiError(404, 'not_found', 'Not found.');
		return server();
	}
	return local as T;
}

/** What `readSearch` stops waiting for the server on. */
const SLOW = Symbol('slow');

/**
 * Whether a failed request to the server says it cannot be reached or is broken, not that the
 * request is wrong.
 */
export const unreachable = (error: unknown) =>
	error instanceof TypeError || (error instanceof ApiError && error.status >= 500);

/**
 * The answer to the API operation `name` of `library` with `params`, for a read the server
 * answers whenever it can be reached: the device answers when this one is offline, the server
 * fails or is slow (`SLOW_MS`), and only if the library is on it. A refusal by the server (a
 * request it finds invalid) is never the device's to second-guess.
 */
export async function readSearch<T>(
	name: string,
	library: string,
	params: object,
	server: () => Promise<T>
): Promise<T> {
	let failure: unknown;
	// The server's answer, once the device answers in its place for being slow.
	let late: Promise<T> | undefined;
	if (connectivity.state === 'online') {
		const asked = server();
		let timer: ReturnType<typeof setTimeout> | undefined;
		const slow = new Promise<typeof SLOW>(
			(resolve) => (timer = setTimeout(resolve, SLOW_MS, SLOW))
		);
		try {
			const answer = await Promise.race([asked, slow]);
			if (answer !== SLOW) return answer;
			// Nothing on the device to hand off to: the server is all there is.
			if (!catalog.onDevice(library)) return await asked;
			late = asked;
		} catch (error) {
			if (!unreachable(error)) throw error;
			failure = error;
		} finally {
			clearTimeout(timer);
		}
	}
	// A server found slow has already found the library on the device.
	if (!late && !(await onDevice(library, failure !== undefined))) {
		if (failure) throw failure;
		if (connectivity.blocked) throw notOnDevice();
		// Unreachable says nothing about HTTP: with no device copy to answer instead, the server
		// is worth asking.
		return server().catch((error) => {
			throw unreachable(error) ? notOnDevice() : error;
		});
	}
	try {
		// Reached only with the server offline, failing or slow, so the device waits for an
		// update under way, or answers from a copy that is not updating, as it does offline.
		const device = catalog.query(library, name, params, true).then((json) => JSON.parse(json) as T);
		// Whichever answers first wins: the slow server may still beat a device that is busy.
		return await (late ? Promise.any([device, late]) : device);
	} catch (error) {
		// Both failed: the server's failure is the one to show.
		if (late) return late;
		if (failure) throw failure;
		console.error(error);
		throw new ApiError(503, 'unavailable_offline', UNAVAILABLE);
	}
}
