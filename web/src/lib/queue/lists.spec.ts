import { describe, expect, it, vi } from 'vitest';
import { download, ListCache } from './lists';

/** A cache whose every call fails, as Cache Storage does in a private window. */
const refusing = {
	open: async () => Promise.reject(new Error('private'))
} as unknown as CacheStorage;

describe('ListCache', () => {
	it('keeps lists in memory where there is no Cache Storage', async () => {
		const lists = new ListCache(undefined, '/api/v1');
		await lists.put('h', '{"tracks":[]}');

		expect(await lists.get('h')).toBe('{"tracks":[]}');
		await lists.keep(new Set());
		expect(await lists.get('h')).toBeNull();
	});

	it('keeps lists in memory when Cache Storage refuses', async () => {
		const lists = new ListCache(refusing, '/api/v1');
		await lists.put('h', 'a');
		await lists.put('i', 'b');
		await lists.delete('i');

		expect(await lists.get('h')).toBe('a');
		expect(await lists.get('i')).toBeNull();
	});
});

describe('ListCache with a Cache Storage that never answers', () => {
	it('falls back to memory after the timeout, and does not wait again', async () => {
		vi.useFakeTimers();
		try {
			const stuck = { open: () => new Promise(() => {}) } as unknown as CacheStorage;
			const lists = new ListCache(stuck, '/api/v1', 100);

			// The first call answers only once the timeout has passed, and gives Cache Storage up.
			const first = lists.get('h');
			await vi.advanceTimersByTimeAsync(100);
			expect(await first).toBeNull();

			// Later calls settle with no timer advanced at all.
			await lists.put('h', 'a');
			expect(await lists.get('h')).toBe('a');
			await lists.keep(new Set(['h']));
			await lists.delete('i');
			expect(await lists.get('j')).toBeNull();
		} finally {
			vi.useRealTimers();
		}
	});
});

describe('download', () => {
	it('tells a missing list from one that could not be reached', async () => {
		const answer = (response: Response | null) => async () => response;

		expect(await download(answer(new Response('x', { status: 404 })), 'h')).toBe('gone');
		expect(await download(answer(new Response('x', { status: 503 })), 'h')).toBe('failed');
		expect(await download(answer(null), 'h')).toBe('failed');
		expect(await download(answer(new Response('{"tracks":[]}')), 'h')).toEqual({
			body: '{"tracks":[]}'
		});
	});
});
