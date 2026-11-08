/// <reference types="@sveltejs/kit" />
/// <reference lib="webworker" />

// The offline shell (`design/general.md` §7.1): the build and the app's page, kept per version, so
// the app starts without the network. It answers nothing from the API: those requests are never
// touched, and what a signed-in account may see is the server's to decide on every request.

import { base, build, files, version } from '$service-worker';

const worker = self as unknown as ServiceWorkerGlobalScope;

const CACHE = `shell-${version}`;
/** The app's one page: the server answers it for every path that is not a file. */
const SHELL = `${base}/`;
// The build also holds a second copy of each wasm module under `workers/assets`, which nothing
// loads: the workers are started with the page's own copy.
const ASSETS = new Set([...build, ...files].filter((path) => !path.includes('/workers/assets/')));

/**
 * Downloads the page and files into this version's cache, and takes over at once, with no
 * prompt: pages of the old version keep working, since their files stay answerable below, and
 * the next load runs this version.
 */
worker.addEventListener('install', (event) => {
	event.waitUntil(
		caches
			.open(CACHE)
			.then((cache) => cache.addAll([new Request(SHELL, { cache: 'reload' }), ...ASSETS]))
			.then(() => worker.skipWaiting())
	);
});

/**
 * Deletes the caches of older versions but the newest one, which pages still open from before
 * this version need for their lazy chunks. Versions are timestamps, so they sort by age; two
 * shells bound the disk the build costs, and a page left open across two upgrades has to reload.
 */
worker.addEventListener('activate', (event) => {
	event.waitUntil(
		caches.keys().then((keys) => {
			const older = keys
				.filter((key) => key.startsWith('shell-') && key !== CACHE)
				.sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
			return Promise.all(older.slice(1).map((key) => caches.delete(key)));
		})
	);
});

/**
 * Answers page loads with this version's page and the app's own files from any version's cache
 * (hashed names never clash), so an old page still loads its chunks; everything else goes to
 * the network untouched.
 */
worker.addEventListener('fetch', (event) => {
	const { request } = event;
	const { origin, pathname } = new URL(request.url);
	if (request.method !== 'GET' || origin !== location.origin || pathname.startsWith(`${base}/api/`))
		return;
	// This version first: a file without a hash in its name, such as the manifest, may differ.
	const current = (path: string) => caches.open(CACHE).then((cache) => cache.match(path));
	const hit =
		request.mode === 'navigate'
			? current(SHELL)
			: ASSETS.has(pathname) || pathname.startsWith(`${base}/_app/immutable/`)
				? current(pathname).then((found) => found ?? caches.match(pathname))
				: null;
	if (hit) event.respondWith(hit.then((found) => found ?? fetch(request)));
});
