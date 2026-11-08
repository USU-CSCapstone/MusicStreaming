// The queue's lists on this device: a store of them by hash, and the download of one
// (`design/queue.md` §8, `design/web.md`, Queue worker). A list never changes under its hash,
// so what is stored is never stale, only unreferenced.

/** The Cache Storage cache the lists live in. */
const CACHE_NAME = 'jewelcase-queue-lists';

/** Deletes every kept list: the next person to sign in must not see the last one's queue. */
export async function clearQueueLists() {
	try {
		await caches.delete(CACHE_NAME);
	} catch (error) {
		console.error(error);
	}
}

/** What a download came to: the list's JSON, `gone` for a `404`, or `failed` for anything else. */
export type Download = { body: string } | 'gone' | 'failed';

/**
 * Lists by hash, as the JSON body of `GET /me/queue/lists/{hash}`, in Cache Storage, which
 * unlike the HTTP cache the browser keeps until told otherwise. Where Cache Storage is missing,
 * refuses, or does not answer within `timeoutMs`, as can happen in a private window, lists are
 * kept in memory instead, and so are lost with the worker; once it times out it is not tried
 * again. Nothing here throws, and nothing waits longer than the timeout.
 */
export class ListCache {
	/** Lists being written, and all of them where Cache Storage cannot be used. */
	#memory = new Map<string, string>();
	#storage: CacheStorage | undefined;
	#prefix: string;
	#timeoutMs: number;
	#cache: Promise<Cache> | null = null;
	#stuck = false;

	/** Makes a store in `storage`, keying each list by the URL it is downloaded from, under `api`. */
	constructor(storage: CacheStorage | undefined, api: string, timeoutMs = 2000) {
		this.#storage = storage;
		this.#prefix = `${api}/me/queue/lists/`;
		this.#timeoutMs = timeoutMs;
	}

	/**
	 * Runs `use` on the cache. The result is undefined where there is no cache, `use` throws, or
	 * the cache did not answer in time, which also gives it up for good.
	 */
	async #run<T>(use: (cache: Cache) => Promise<T>): Promise<T | undefined> {
		const storage = this.#storage;
		if (!storage || this.#stuck) return undefined;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const timeout = new Promise<'timeout'>((resolve) => {
			timer = setTimeout(() => resolve('timeout'), this.#timeoutMs);
		});
		try {
			this.#cache ??= storage.open(CACHE_NAME);
			const done = this.#cache.then(use);
			done.catch(() => {});
			const result = await Promise.race([done, timeout]);
			if (result === 'timeout') this.#stuck = true;
			else return result;
		} catch {
			// Treated as no cache for this call.
		} finally {
			clearTimeout(timer);
		}
		return undefined;
	}

	/** The list under `hash`, or null if it is not held. */
	async get(hash: string): Promise<string | null> {
		const held = this.#memory.get(hash);
		if (held !== undefined) return held;
		return (
			(await this.#run(async (cache) => (await cache.match(this.#prefix + hash))?.text())) ?? null
		);
	}

	/** Keeps `body` under `hash`. It can be read back at once, before the write is done. */
	async put(hash: string, body: string): Promise<void> {
		this.#memory.set(hash, body);
		const done = await this.#run(async (cache) => {
			await cache.put(this.#prefix + hash, new Response(body));
			return true;
		});
		if (done) this.#memory.delete(hash);
	}

	/** Forgets the list under `hash`. */
	async delete(hash: string): Promise<void> {
		this.#memory.delete(hash);
		await this.#run((cache) => cache.delete(this.#prefix + hash));
	}

	/**
	 * Forgets every list but those in `hashes`. After a reload nothing else knows what was
	 * cached, and lists share the site's quota with downloaded music.
	 */
	async keep(hashes: Set<string>): Promise<void> {
		for (const hash of this.#memory.keys()) if (!hashes.has(hash)) this.#memory.delete(hash);
		await this.#run(async (cache) => {
			for (const request of await cache.keys()) {
				const hash = request.url.slice(request.url.lastIndexOf('/') + 1);
				if (!hashes.has(hash)) await cache.delete(request);
			}
		});
	}
}

/**
 * Downloads the list under `hash` with `get`, which answers a path under the API or null if it
 * could not be made. A `404` means the server will not give it, now or later.
 */
export async function download(
	get: (path: string) => Promise<Response | null>,
	hash: string
): Promise<Download> {
	const response = await get(`/me/queue/lists/${hash}`);
	if (response?.status === 404) return 'gone';
	if (!response?.ok) return 'failed';
	try {
		return { body: await response.text() };
	} catch {
		return 'failed';
	}
}
