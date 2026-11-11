/** The signature of `fetch`. */
type Fetch = typeof fetch;

/**
 * `real` (the global `fetch` by default) unless `blocked()` says no request may leave, when the
 * request fails as one with no network does, with a `TypeError`. How Work offline stops the
 * requests of the page and its workers (`connectivity.svelte.ts`).
 */
export const gated =
	(blocked: () => boolean, real: Fetch = (...args) => fetch(...args)): Fetch =>
	(input, init) =>
		blocked() ? Promise.reject(new TypeError('Work offline is on')) : real(input, init);
