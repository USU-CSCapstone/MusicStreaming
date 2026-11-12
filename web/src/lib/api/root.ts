// Where the API is, where to log in and what a refusal looks like: shared by the modules
// `client.ts` itself depends on, which cannot import it back.

import { base, resolve } from '$app/paths';

/** The API's base path. */
export const API = `${base}/api/v1`;

/** The login page, returning to where the browser is now once logged in. */
export const loginUrl = () =>
	`${resolve('/login')}?${new URLSearchParams({ next: location.pathname + location.search })}`;

/** What a page or an action that needs the server says when this device cannot reach it. */
export const UNAVAILABLE_OFFLINE =
	'Unavailable offline. This needs the server, which this device can’t reach right now.';

/** A refusal or failure from the API, with its HTTP status and machine-readable `code`. */
export class ApiError extends Error {
	constructor(
		readonly status: number,
		readonly code: string | undefined,
		message: string,
		/** Seconds until trying again is worth it, with `rate_limited`. */
		readonly retryAfter?: number
	) {
		super(message);
	}
}
