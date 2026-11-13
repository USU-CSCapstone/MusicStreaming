// The device's copy of the images an account saw online, so their art shows offline. Kept in
// Cache Storage, one cache for each account, by the service worker, which is the only thing that
// adds to it, and only once a page has told it who is signed in; the pages read its size and
// empty it at sign-out. Images are the one API response that is kept: their IDs are
// content-addressed, so a kept answer is never stale.

/** The start of every account's cache's name. */
export const IMAGE_PREFIX = 'images-';

/** The name of `user`'s cache of images. */
export const imageCache = (user: string) => `${IMAGE_PREFIX}${user}`;

/**
 * The cache, in the service worker's own Cache Storage, that remembers which account is signed in.
 */
export const SIGNED_IN_CACHE = 'jewelcase-signed-in';

/**
 * The most images kept. A cover at the sizes the app asks for is about 5 to 60 KB, so this is
 * tens of megabytes at most: several screens of a large library's art, well within what a phone
 * allows a web app, while the oldest are dropped before the device's storage is a concern.
 */
export const MAX_IMAGES = 2000;

/**
 * Whether `url` is an image's: `{api}/libraries/{id}/images/{id}` with at most a `size`. Nothing
 * else is kept. The API's path is passed in because the service worker cannot import `$app`.
 */
export function isImageUrl({ pathname, searchParams }: URL, api: string): boolean {
	const prefix = `${api}/libraries/`;
	if (!pathname.startsWith(prefix)) return false;
	const parts = pathname.slice(prefix.length).split('/');
	return (
		parts.length === 3 &&
		parts[1] === 'images' &&
		parts.every(Boolean) &&
		[...searchParams.keys()].every((key) => key === 'size')
	);
}

/** Deletes the oldest entries beyond `max`: a cache lists its keys oldest first. */
export async function trimImages(cache: Pick<Cache, 'keys' | 'delete'>, max = MAX_IMAGES) {
	const keys = await cache.keys();
	await Promise.all(keys.slice(0, Math.max(0, keys.length - max)).map((key) => cache.delete(key)));
}

/**
 * How many of `user`'s images are kept and their bytes; zero where the browser has no Cache
 * Storage.
 */
export async function imageCacheSize(user: string): Promise<{ count: number; bytes: number }> {
	try {
		if (!(await caches.has(imageCache(user)))) return { count: 0, bytes: 0 };
		const cache = await caches.open(imageCache(user));
		const sizes = await Promise.all(
			(await cache.keys()).map(async (key) => (await (await cache.match(key))?.blob())?.size ?? 0)
		);
		return { count: sizes.length, bytes: sizes.reduce((sum, size) => sum + size, 0) };
	} catch {
		return { count: 0, bytes: 0 };
	}
}

/**
 * Deletes every account's kept images: the next person to sign in must not see the last one's art.
 */
export async function clearImageCache() {
	try {
		for (const name of await caches.keys())
			if (name.startsWith(IMAGE_PREFIX)) await caches.delete(name);
	} catch (error) {
		console.error(error);
	}
}

/**
 * Deletes the images of `library` kept for `user`, which the account no longer reaches; `api` is
 * the API's path.
 */
export async function dropLibraryImages(user: string, library: string, api: string) {
	try {
		const cache = await caches.open(imageCache(user));
		const prefix = `${api}/libraries/${library}/images/`;
		for (const request of await cache.keys())
			if (new URL(request.url).pathname.startsWith(prefix)) await cache.delete(request);
	} catch (error) {
		console.error(error);
	}
}

/**
 * Who the service worker was last told is signed in, which one that takes over the page later is
 * told too.
 */
let told: string | null = null;
/** Whether `controllerchange` is already listened for, so it is added once. */
let listening = false;

/** Posts `told` to the active service worker, if there is one. */
const sendUser = async () =>
	(await navigator.serviceWorker.getRegistration())?.active?.postMessage({
		type: 'user',
		user: told
	});

/**
 * Tells the service worker which account is signed in, or `null` for none: it keeps images only
 * for one it knows.
 */
export async function tellServiceWorker(user: string | null) {
	told = user;
	if (!('serviceWorker' in navigator)) return;
	if (!listening) {
		listening = true;
		navigator.serviceWorker.addEventListener('controllerchange', () => void sendUser());
	}
	await sendUser().catch(console.error);
}

/**
 * The caches `activate` deletes: other versions' shells. Never the images, which outlive versions.
 */
export const oldShells = (keys: string[], current: string) =>
	keys.filter((key) => key.startsWith('shell-') && key !== current);
