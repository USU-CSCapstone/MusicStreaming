import { afterEach, describe, expect, it, vi } from 'vitest';
import { clearQueueLists, download, dropLibraryLists, ListCache } from './lists';

/** A cache whose every call fails, as Cache Storage does in a private window. */
const refusing = {
	open: async () => Promise.reject(new Error('private'))
} as unknown as CacheStorage;

describe('ListCache', () => {
	it('keeps lists in memory where there is no Cache Storage', async () => {
		const lists = new ListCache(undefined, '/api/v1', '1');
		await lists.put('h', '{"tracks":[]}');

		expect(await lists.get('h')).toBe('{"tracks":[]}');
		await lists.keep(new Set());
		expect(await lists.get('h')).toBeNull();
	});

	it('keeps lists in memory when Cache Storage refuses', async () => {
		const lists = new ListCache(refusing, '/api/v1', '1');
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
			const lists = new ListCache(stuck, '/api/v1', '1', 100);

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

/**
 * A Cache Storage with `held` caches, each mapping a URL to a body, and the calls the module makes.
 */
function fakeStorage(held: Record<string, Record<string, string>>) {
	const stores = new Map(Object.entries(held).map(([name, lists]) => [name, { ...lists }]));
	const storage = {
		keys: async () => [...stores.keys()],
		delete: async (name: string) => stores.delete(name),
		open: async (name: string) => {
			const lists = stores.get(name) ?? stores.set(name, {}).get(name)!;
			return {
				keys: async () => Object.keys(lists).map((url) => new Request(`http://x${url}`)),
				match: async (request: Request | string) => {
					const path = typeof request === 'string' ? request : new URL(request.url).pathname;
					return path in lists ? new Response(lists[path]) : undefined;
				},
				put: async (path: string, response: Response) => void (lists[path] = await response.text()),
				delete: async (request: Request | string) => {
					const path = typeof request === 'string' ? request : new URL(request.url).pathname;
					return delete lists[path];
				}
			};
		}
	};
	vi.stubGlobal('caches', storage);
	return { stores, storage: storage as unknown as CacheStorage };
}

describe('the lists kept for each account', () => {
	afterEach(() => vi.unstubAllGlobals());

	it('are in a cache of the account’s own', async () => {
		const { stores, storage } = fakeStorage({});
		await new ListCache(storage, '/api/v1', '1').put('h', '{}');
		expect(await new ListCache(storage, '/api/v1', '2').get('h')).toBeNull();
		expect(Object.keys(stores.get('queue-lists-1')!)).toHaveLength(1);
	});

	it('are deleted for every account at sign-out', async () => {
		const { stores } = fakeStorage({ 'queue-lists-1': {}, 'queue-lists-2': {}, other: {} });
		await clearQueueLists();
		expect([...stores.keys()]).toEqual(['other']);
	});

	it('are dropped for a library the account lost, and only for that library and account', async () => {
		const { stores } = fakeStorage({
			'queue-lists-1': {
				'/a': JSON.stringify({ libraryId: '7', tracks: [] }),
				'/b': JSON.stringify({ libraryId: '8', tracks: [] }),
				'/c': 'not json'
			},
			'queue-lists-2': { '/a': JSON.stringify({ libraryId: '7', tracks: [] }) }
		});
		await dropLibraryLists('1', '7');
		expect(Object.keys(stores.get('queue-lists-1')!)).toEqual(['/b', '/c']);
		expect(Object.keys(stores.get('queue-lists-2')!)).toEqual(['/a']);
	});
});
