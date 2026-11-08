import { beforeEach, describe, expect, it, vi } from 'vitest';

type Handler = (event: Record<string, unknown>) => void;

/** A cache storage that keeps each cache's files by path and, like the browser's, matches across caches oldest first. */
function fakeCaches() {
	const stores = new Map<string, Map<string, string>>();
	const cache = (files: Map<string, string>) => ({
		addAll: async (items: ({ url: string } | string)[]) => {
			for (const item of items) files.set(typeof item === 'string' ? item : item.url, 'file');
		},
		put: async (path: string, body: string) => void files.set(path, body),
		match: async (path: string) => files.get(path)
	});
	return {
		stores,
		open: async (name: string) => {
			if (!stores.has(name)) stores.set(name, new Map());
			return cache(stores.get(name)!);
		},
		match: async (path: string) => {
			for (const files of stores.values()) if (files.has(path)) return files.get(path);
		},
		keys: async () => [...stores.keys()],
		delete: async (name: string) => stores.delete(name)
	};
}

/** Loads the worker as version `version` of the build, which holds `build`, against `caches`. */
async function load(version: string, build: string[], caches: ReturnType<typeof fakeCaches>) {
	vi.resetModules();
	const handlers: Record<string, Handler> = {};
	const skipWaiting = vi.fn(async () => {});
	vi.stubGlobal('self', {
		addEventListener: (type: string, handler: Handler) => (handlers[type] = handler),
		skipWaiting
	});
	vi.stubGlobal('location', { origin: 'https://app.test' });
	vi.stubGlobal('caches', caches);
	vi.stubGlobal(
		'Request',
		class {
			constructor(public url: string) {}
		}
	);
	vi.stubGlobal('fetch', async () => 'network');
	vi.doMock('$service-worker', () => ({ base: '', build, files: [], version }));
	// By a variable, so the worker's own `webworker` types stay out of the page's program.
	const path = './service-worker';
	await import(/* @vite-ignore */ path);

	/** Runs the lifecycle event `type`, waiting for what it asks to be kept alive. */
	const run = async (type: string) => {
		let wait: Promise<unknown> = Promise.resolve();
		handlers[type]({ waitUntil: (promise: Promise<unknown>) => (wait = promise) });
		await wait;
	};
	/** What the worker answers a request for `path` with, or `undefined` if it leaves it alone. */
	const answer = async (path: string, mode = 'cors') => {
		let reply: Promise<unknown> | undefined;
		handlers.fetch({
			request: { url: `https://app.test${path}`, method: 'GET', mode },
			respondWith: (promise: Promise<unknown>) => (reply = promise)
		});
		return reply;
	};
	return { run, answer, skipWaiting };
}

let caches: ReturnType<typeof fakeCaches>;

beforeEach(() => {
	caches = fakeCaches();
});

describe('the service worker across versions', () => {
	it('takes over as soon as it has installed', async () => {
		const worker = await load('100', ['/_app/immutable/a.js'], caches);
		await worker.run('install');
		expect(worker.skipWaiting).toHaveBeenCalledOnce();
	});

	it('answers a page of the old version its files, and a navigation with the new page', async () => {
		const old = await load('100', ['/_app/immutable/old.js'], caches);
		await old.run('install');
		(await caches.open('shell-100')).put('/', 'old page');
		const next = await load('200', ['/_app/immutable/new.js'], caches);
		await next.run('install');
		(await caches.open('shell-200')).put('/', 'new page');
		await next.run('activate');

		expect(await next.answer('/_app/immutable/old.js')).toBe('file');
		expect(await next.answer('/_app/immutable/new.js')).toBe('file');
		expect(await next.answer('/albums', 'navigate')).toBe('new page');
		expect(await next.answer('/api/v1/me')).toBeUndefined();
	});

	it('keeps this version’s caches and the newest older one, and no more', async () => {
		for (const version of ['100', '90', '1000']) await caches.open(`shell-${version}`);
		await caches.open('images');
		const worker = await load('2000', [], caches);
		await worker.run('install');
		await worker.run('activate');
		expect([...caches.stores.keys()].sort()).toEqual(['images', 'shell-1000', 'shell-2000']);
	});
});
