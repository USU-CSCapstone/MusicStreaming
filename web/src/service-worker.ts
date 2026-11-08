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
 * Downloads the page and files into this version's cache. A new version installs beside the old
 * one and waits, with no `skipWaiting` or `claim`: it takes over when the old one's last tab
 * closes, so an open app is never reloaded under its user.
 */
worker.addEventListener('install', (event) => {
	event.waitUntil(
		caches
			.open(CACHE)
			.then((cache) => cache.addAll([new Request(SHELL, { cache: 'reload' }), ...ASSETS]))
	);
});

/** Deletes the caches of other versions once this one takes over. */
worker.addEventListener('activate', (event) => {
	event.waitUntil(
		caches
			.keys()
			.then((keys) =>
				Promise.all(keys.filter((key) => key !== CACHE).map((key) => caches.delete(key)))
			)
	);
});

/** Answers page loads with the kept page and the app's own files from the cache; everything else goes to the network untouched. */
worker.addEventListener('fetch', (event) => {
	const { request } = event;
	const { origin, pathname } = new URL(request.url);
	if (request.method !== 'GET' || origin !== location.origin || pathname.startsWith(`${base}/api/`))
		return;
	const cached = request.mode === 'navigate' ? SHELL : ASSETS.has(pathname) ? pathname : null;
	if (cached !== null) event.respondWith(caches.match(cached).then((hit) => hit ?? fetch(request)));
});
