import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	clearImageCache,
	dropLibraryImages,
	imageCache,
	isImageUrl,
	oldShells,
	trimImages
} from './image-cache';

/** A cache holding `urls`, oldest first, as Cache Storage lists its keys. */
function fakeCache(urls: string[]) {
	const kept = [...urls];
	return {
		kept,
		keys: async () => kept.map((url) => new Request(`http://x/${url}`)),
		delete: async (request: RequestInfo | URL) => {
			const at = kept.indexOf((request as Request).url.replace('http://x/', ''));
			if (at >= 0) kept.splice(at, 1);
			return at >= 0;
		}
	};
}

describe('the image cache', () => {
	it('the cache evicts the oldest beyond its bound', async () => {
		const cache = fakeCache(['a', 'b', 'c', 'd', 'e']);
		await trimImages(cache, 3);
		expect(cache.kept).toEqual(['c', 'd', 'e']);
	});

	it('keeps everything within its bound', async () => {
		const cache = fakeCache(['a', 'b']);
		await trimImages(cache, 2);
		expect(cache.kept).toEqual(['a', 'b']);
	});

	it('keeps only images, matched exactly', () => {
		const is = (path: string) => isImageUrl(new URL(path, 'http://x'), '/api/v1');
		expect(is('/api/v1/libraries/1/images/9?size=96')).toBe(true);
		expect(is('/api/v1/libraries/1/images/9')).toBe(true);
		expect(is('/api/v1/libraries/1/images/9?size=96&token=x')).toBe(false);
		expect(is('/api/v1/libraries/1/images/9/extra')).toBe(false);
		expect(is('/api/v1/libraries/1/images/')).toBe(false);
		expect(is('/api/v1/libraries/1/albums')).toBe(false);
		expect(is('/api/v1/libraries/1/albums/9')).toBe(false);
		expect(is('/api/v1/me')).toBe(false);
		expect(is('/other/api/v1/libraries/1/images/9')).toBe(false);
	});

	it('a new version deletes only the old shells, never the images', () => {
		expect(oldShells(['shell-2', 'shell-1', imageCache('1'), 'other'], 'shell-2')).toEqual([
			'shell-1'
		]);
	});
});

/** A Cache Storage holding `names`' caches, each with `urls`, with the calls the module makes. */
function fakeStorage(held: Record<string, string[]>) {
	const stores = new Map(Object.entries(held).map(([name, urls]) => [name, [...urls]]));
	const cache = (name: string) => ({
		keys: async () => stores.get(name)!.map((url) => new Request(`http://x${url}`)),
		delete: async (request: Request) => {
			const urls = stores.get(name)!;
			urls.splice(urls.indexOf(new URL(request.url).pathname), 1);
			return true;
		}
	});
	vi.stubGlobal('caches', {
		keys: async () => [...stores.keys()],
		delete: async (name: string) => stores.delete(name),
		open: async (name: string) => cache(name)
	});
	return stores;
}

afterEach(() => vi.unstubAllGlobals());

describe('what the device keeps of each account’s art', () => {
	it('is a cache of its own for each account', () => {
		expect(imageCache('1')).not.toBe(imageCache('2'));
	});

	it('is deleted for every account, and nothing else, at sign-out', async () => {
		const stores = fakeStorage({
			[imageCache('1')]: ['/a'],
			[imageCache('2')]: ['/b'],
			'shell-1': []
		});
		await clearImageCache();
		expect([...stores.keys()]).toEqual(['shell-1']);
	});

	it('is dropped for a library the account lost, and only for that library and account', async () => {
		const stores = fakeStorage({
			[imageCache('1')]: [
				'/api/v1/libraries/7/images/a',
				'/api/v1/libraries/70/images/b',
				'/api/v1/libraries/8/images/c'
			],
			[imageCache('2')]: ['/api/v1/libraries/7/images/a']
		});
		await dropLibraryImages('1', '7', '/api/v1');
		expect(stores.get(imageCache('1'))).toEqual([
			'/api/v1/libraries/70/images/b',
			'/api/v1/libraries/8/images/c'
		]);
		expect(stores.get(imageCache('2'))).toEqual(['/api/v1/libraries/7/images/a']);
	});
});
