import { beforeEach, describe, expect, it, vi } from 'vitest';

/** A listener the worker adds, called with a fake event. */
type Handler = (event: Record<string, unknown>) => void;

/**
 * A cache storage that keeps each cache's files by path and, like the browser's, matches across
 * caches oldest first. While `offline` is set, a download fails.
 */
function fakeCaches() {
	const stores = new Map<string, Map<string, string | Response>>();
	const cache = (files: Map<string, string | Response>) => ({
		addAll: async (items: ({ url: string } | string)[]) => {
			if (network.offline) throw new TypeError('Failed to fetch');
			for (const item of items) files.set(typeof item === 'string' ? item : item.url, 'file');
		},
		put: async (path: string, body: string | Response) => void files.set(path, body),
		match: async (path: string) => files.get(path),
		keys: async () => [...files.keys()].map((url) => ({ url })),
		delete: async (request: { url: string }) => files.delete(request.url)
	});
	const network = { offline: false };
	return {
		stores,
		network,
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
		skipWaiting,
		clients: { claim: async () => {} }
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

	/** Runs the event `type` with `data`, waiting for what it asks to be kept alive. */
	const run = async (type: string, data?: unknown) => {
		let wait: Promise<unknown> = Promise.resolve();
		handlers[type]({ data, waitUntil: (promise: Promise<unknown>) => (wait = promise) });
		await wait;
	};
	/** What the worker answers a request for `path` with, or `undefined` if it leaves it alone. */
	const answer = async (path: string, mode = 'cors') => {
		let reply: Promise<unknown> | undefined;
		let kept: Promise<unknown> = Promise.resolve();
		handlers.fetch({
			request: { url: `https://app.test${path}`, method: 'GET', mode, headers: new Headers() },
			respondWith: (promise: Promise<unknown>) => (reply = promise),
			waitUntil: (promise: Promise<unknown>) => (kept = promise)
		});
		const response = await reply;
		await kept;
		return response;
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
		const old = await load('100', ['/_app/immutable/old.js', '/manifest.webmanifest'], caches);
		await old.run('install');
		(await caches.open('shell-100')).put('/', 'old page');
		(await caches.open('shell-100')).put('/manifest.webmanifest', 'old manifest');
		const next = await load('200', ['/_app/immutable/new.js', '/manifest.webmanifest'], caches);
		await next.run('install');
		(await caches.open('shell-200')).put('/', 'new page');
		(await caches.open('shell-200')).put('/manifest.webmanifest', 'new manifest');
		await next.run('activate');

		expect(await next.answer('/_app/immutable/old.js')).toBe('file');
		// A file without a hash in its name is this version's.
		expect(await next.answer('/manifest.webmanifest')).toBe('new manifest');
		expect(await next.answer('/_app/immutable/new.js')).toBe('file');
		expect(await next.answer('/albums', 'navigate')).toBe('new page');
		expect(await next.answer('/api/v1/me')).toBeUndefined();
	});

	it('keeps this version’s caches and the newest older one, and no more', async () => {
		for (const version of ['100', '90', '1000']) await caches.open(`shell-${version}`);
		await caches.open('images-1');
		const worker = await load('2000', [], caches);
		await worker.run('install');
		await worker.run('activate');
		expect([...caches.stores.keys()].sort()).toEqual(['images-1', 'shell-1000', 'shell-2000']);
	});

	it('keeps no empty shell from an update that failed to download, so the next keeps the one pages run', async () => {
		await (await load('100', ['/_app/immutable/a.js'], caches)).run('install');
		caches.network.offline = true;
		await expect((await load('200', [], caches)).run('install')).rejects.toThrow();
		expect([...caches.stores.keys()]).toEqual(['shell-100']);

		caches.network.offline = false;
		const next = await load('300', [], caches);
		await next.run('install');
		await next.run('activate');
		expect([...caches.stores.keys()].sort()).toEqual(['shell-100', 'shell-300']);
	});
});

describe('the service worker’s images', () => {
	const IMAGE = '/api/v1/libraries/1/images/9';

	/** A page's message saying `user` is signed in, or no one. */
	const tell = (worker: Awaited<ReturnType<typeof load>>, user: string | null) =>
		worker.run('message', { type: 'user', user });

	/** A network that answers every image with a body of its own, counting its requests. */
	function network() {
		const asked: string[] = [];
		vi.stubGlobal('fetch', async (request: { url: string }) => {
			asked.push(request.url);
			return Object.assign(new Response(`art ${asked.length}`), {
				clone: () => new Response('copy')
			});
		});
		return asked;
	}

	it('keeps an image under the signed-in account’s own cache, and answers it from there next time', async () => {
		const worker = await load('100', [], caches);
		const asked = network();
		await tell(worker, '1');
		await worker.answer(IMAGE);
		expect([...caches.stores.keys()]).toContain('images-1');
		expect([...caches.stores.get('images-1')!.keys()]).toEqual(['https://app.test' + IMAGE]);

		await worker.answer(IMAGE);
		expect(asked).toHaveLength(1);
	});

	it('goes to the network for what another account kept, and keeps nothing for one it does not know', async () => {
		const worker = await load('100', [], caches);
		const asked = network();
		await tell(worker, '1');
		await worker.answer(IMAGE);

		await tell(worker, '2');
		// The first account's art is gone as the second is told of, and was never its to be given.
		expect([...caches.stores.keys()]).not.toContain('images-1');
		await worker.answer(IMAGE);
		expect(asked).toHaveLength(2);

		await tell(worker, null);
		expect([...caches.stores.keys()].filter((name) => name.startsWith('images-'))).toEqual([]);
		await worker.answer(IMAGE);
		expect(asked).toHaveLength(3);
		expect([...caches.stores.keys()].filter((name) => name.startsWith('images-'))).toEqual([]);
	});

	it('keeps nothing before any page has said who is signed in', async () => {
		const worker = await load('100', [], caches);
		const asked = network();
		await worker.answer(IMAGE);
		expect(asked).toHaveLength(1);
		expect([...caches.stores.keys()].filter((name) => name.startsWith('images-'))).toEqual([]);
	});

	it('remembers who is signed in across its own restarts', async () => {
		await tell(await load('100', [], caches), '1');
		const restarted = await load('100', [], caches);
		network();
		await restarted.answer(IMAGE);
		expect([...caches.stores.keys()]).toContain('images-1');
	});

	it('deletes the unscoped cache an older build kept', async () => {
		for (const name of ['jewelcase-queue-lists', 'images-1']) await caches.open(name);
		const worker = await load('100', [], caches);
		await worker.run('activate');
		expect([...caches.stores.keys()]).toEqual(['images-1']);
	});
});
