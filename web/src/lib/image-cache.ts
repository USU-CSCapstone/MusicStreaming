// The device's copy of the images seen online, so their art shows offline. Kept in Cache Storage
// by the service worker, which is the only thing that adds to it; the pages read its size and
// empty it at sign-out. Images are the one API response that is kept: their IDs are
// content-addressed, so a kept answer is never stale.

export const IMAGE_CACHE = 'images';

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

/** How many images are kept and their bytes; zero where the browser has no Cache Storage. */
export async function imageCacheSize(): Promise<{ count: number; bytes: number }> {
	try {
		if (!(await caches.has(IMAGE_CACHE))) return { count: 0, bytes: 0 };
		const cache = await caches.open(IMAGE_CACHE);
		const sizes = await Promise.all(
			(await cache.keys()).map(async (key) => (await (await cache.match(key))?.blob())?.size ?? 0)
		);
		return { count: sizes.length, bytes: sizes.reduce((sum, size) => sum + size, 0) };
	} catch {
		return { count: 0, bytes: 0 };
	}
}

/** Deletes every kept image: the next person to sign in must not see the last one's art. */
export async function clearImageCache() {
	try {
		await caches.delete(IMAGE_CACHE);
	} catch (error) {
		console.error(error);
	}
}

/** The caches `activate` deletes: other versions' shells. Never the images, which outlive versions. */
export const oldShells = (keys: string[], current: string) =>
	keys.filter((key) => key.startsWith('shell-') && key !== current);
