/// <reference types="@sveltejs/kit" />
/// <reference lib="webworker" />

// The offline shell (`design/general.md` §7.1): the build and the app's page, kept per version, so
// the app starts without the network. It answers nothing from the API but images: other requests
// are never touched, and what a signed-in account may see is the server's to decide on every
// request. Images are kept as they are seen (`$lib/image-cache`), oldest out first.

import { base, build, files, version } from '$service-worker';
import { IMAGE_CACHE, isImageUrl, oldShells, trimImages } from '$lib/image-cache';

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
 * Deletes the shells of older versions but the newest one, which pages still open from before
 * this version need for their lazy chunks; the images outlive versions. Versions are timestamps,
 * so they sort by age; two shells bound the disk the build costs, and a page left open across
 * two upgrades has to reload. Then takes over the pages already open, so a first visit's pages
 * not yet seen load offline.
 */
worker.addEventListener('activate', (event) => {
	event.waitUntil(
		caches
			.keys()
			.then((keys) => {
				const older = oldShells(keys, CACHE).sort((a, b) =>
					b.localeCompare(a, undefined, { numeric: true })
				);
				return Promise.all(older.slice(1).map((key) => caches.delete(key)));
			})
			.then(() => worker.clients.claim())
	);
});

let trimming: Promise<void> | undefined;

/**
 * An image from the cache if it was seen before, else from the network, keeping what the network
 * answers in full. Offline and unseen, the request fails and the page shows its placeholder.
 */
async function image(event: FetchEvent): Promise<Response> {
	const cache = await caches.open(IMAGE_CACHE);
	const { url } = event.request;
	const hit = await cache.match(url);
	if (hit) return hit;
	const response = await fetch(event.request);
	if (response.status === 200)
		event.waitUntil(
			cache.put(url, response.clone()).then(() => {
				// A burst of covers shares one listing of the cache; the bound is soft for a moment.
				trimming ??= trimImages(cache).finally(() => (trimming = undefined));
				return trimming;
			})
		);
	return response;
}

/**
 * Answers images from the cache or the network, page loads with this version's page, and the
 * app's own files from any version's cache (hashed names never clash), so an old page still
 * loads its chunks; everything else goes to the network untouched.
 */
worker.addEventListener('fetch', (event) => {
	const { request } = event;
	const target = new URL(request.url);
	const { origin, pathname } = target;
	if (request.method !== 'GET' || origin !== location.origin) return;
	if (isImageUrl(target, `${base}/api/v1`) && !request.headers.has('range'))
		return event.respondWith(image(event));
	if (pathname.startsWith(`${base}/api/`)) return;
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
