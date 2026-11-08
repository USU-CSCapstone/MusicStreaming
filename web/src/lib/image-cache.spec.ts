import { describe, expect, it } from 'vitest';
import { IMAGE_CACHE, isImageUrl, oldShells, trimImages } from './image-cache';

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
		expect(oldShells(['shell-2', 'shell-1', IMAGE_CACHE, 'other'], 'shell-2')).toEqual(['shell-1']);
	});
});
