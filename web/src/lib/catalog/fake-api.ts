// A fake server for the catalog sync's tests: the library list, each library's snapshot and
// change feed, and the account feed, held in memory and served as `fetch` does. A test sets
// what the server holds and how it misbehaves, and reads what was asked.

import { schemaVersion } from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';

type Change = { seq: number; op: string; kind: string; id: string; key?: number; rows?: object };
type Feed = { changes: Change[]; head: number };

/** The time to wait for a held response, so a test that forgets to release one fails. */
const HOLD_MS = 10_000;

/** A JSON error as the API sends one. */
const problem = (status: number, headers: Record<string, string> = {}) =>
	new Response(JSON.stringify({ code: String(status) }), {
		status,
		headers: { 'content-type': 'application/problem+json', ...headers }
	});

export class FakeApi {
	/** Every path asked, with its query, in order. */
	requests: string[] = [];

	#libraries: string[] = [];
	#session = '1';
	#snapshot: Uint8Array = new Uint8Array(0);
	#seq = new Map<string, number>();
	#rebuilds = new Map<string, number>();
	#schema: number | undefined;
	#etag = '';
	#feeds = new Map<string, Feed>();
	#account: Feed = { changes: [], head: 0 };
	#pageSize = 1000;
	#failures: { match: string; after: number; status: number }[] = [];
	#building = 0;
	#wrongEtags = 0;
	#bodyHeld: { release: () => void; reached: boolean } | undefined;
	#held: { match: string; release: () => void; reached: boolean } | undefined;

	/** Serves `bytes` as every library's snapshot. */
	async setSnapshot(bytes: Uint8Array) {
		this.#snapshot = bytes;
		const hash = await crypto.subtle.digest('SHA-256', bytes as BufferSource);
		this.#etag = [...new Uint8Array(hash)].map((b) => b.toString(16).padStart(2, '0')).join('');
	}

	/** The library list. */
	setLibraries(ids: string[]) {
		this.#libraries = ids;
	}

	/** The account the session cookie is signed in as. */
	setSession(user: string) {
		this.#session = user;
	}

	/** The feed position the snapshot of `library` is current to. */
	setSeq(library: string, seq: number) {
		this.#seq.set(library, seq);
	}

	/**
	 * Moves the snapshot of `library` on to `seq` after the next time it is asked for, as the
	 * server serves a stale snapshot once more while that request rebuilds it.
	 */
	rebuild(library: string, seq: number) {
		this.#rebuilds.set(library, seq);
	}

	/** Serves snapshots as of device schema version `version`, rather than this build's. */
	setSchema(version: number) {
		this.#schema = version;
	}

	/** The library's change feed: `changes` in order, and the position of its latest. */
	setFeed(library: string, changes: Change[], head: number) {
		this.#feeds.set(library, { changes, head });
	}

	/** The account's change feed. */
	setAccount(changes: Change[], head: number) {
		this.#account = { changes, head };
	}

	/** Returns at most this many changes a page, whatever limit is asked. */
	setPageSize(size: number) {
		this.#pageSize = size;
	}

	/**
	 * Answers requests whose path contains `match` with `status`, once `after` have been answered.
	 */
	failAfter(match: string, after: number, status = 500) {
		this.#failures.push({ match, after, status });
	}

	/** Stops failing. */
	healed() {
		this.#failures = [];
	}

	/** Answers the next `times` snapshot requests `503 catalog_building`. */
	building(times: number) {
		this.#building = times;
	}

	/** Gives the next `times` snapshots an ETag that is not their hash. */
	wrongEtag(times: number) {
		this.#wrongEtags = times;
	}

	/** Holds the answer to the next request whose path contains `match`, until `release`. */
	hold(match: string) {
		this.#held = { match, release: () => {}, reached: false };
	}

	/** Stops the next snapshot's body after its first piece, until `releaseBody`. */
	holdBody() {
		this.#bodyHeld = { release: () => {}, reached: false };
	}

	/** Whether a snapshot's body is stopped. */
	bodyHeld() {
		return this.#bodyHeld?.reached ?? false;
	}

	/** Lets the stopped body go on. */
	releaseBody() {
		this.#bodyHeld?.release();
		this.#bodyHeld = undefined;
	}

	/** Whether a request is held. */
	holding() {
		return this.#held?.reached ?? false;
	}

	/** Lets the held answer go. */
	release() {
		this.#held?.release();
		this.#held = undefined;
	}

	/** The paths asked that contain `match`. */
	asked(match: string) {
		return this.requests.filter((path) => path.includes(match));
	}

	/** Answers a request as the API would; the signature is `fetch`'s. */
	fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
		const url = new URL(String(input), 'http://localhost');
		const path = url.pathname.replace(/^\/api\/v1/, '') + url.search;
		this.requests.push(path);
		const failure = this.#failures.find(
			(f) =>
				path.includes(f.match) && this.requests.filter((r) => r.includes(f.match)).length > f.after
		);
		// Answered when asked, though delivered when released.
		const response = failure ? problem(failure.status) : this.#answer(url);
		const held = this.#held;
		if (held && path.includes(held.match) && !held.reached) {
			held.reached = true;
			await new Promise<void>((resolve, reject) => {
				held.release = resolve;
				setTimeout(() => reject(new Error('a held request was never released')), HOLD_MS);
				init?.signal?.addEventListener('abort', () => reject(new DOMException('', 'AbortError')), {
					once: true
				});
			});
		}
		return response;
	};

	/** The answer to a request that was not made to fail. */
	#answer(url: URL): Response {
		const path = url.pathname.replace(/^\/api\/v1/, '');
		const json = (body: object) => Response.json(body);
		if (path === '/libraries') return json({ items: this.#libraries.map((id) => ({ id })) });
		if (path === '/me') return json({ id: this.#session });
		if (path === '/me/changes') return this.#page(this.#account, url);
		const library = /^\/libraries\/(\d+)\/(catalog|changes)$/.exec(path);
		if (!library || !this.#libraries.includes(library[1])) return problem(404);
		if (library[2] === 'changes') {
			const feed = this.#feeds.get(library[1]);
			return feed ? this.#page(feed, url) : problem(404);
		}
		if (this.#building > 0) {
			this.#building--;
			return problem(503, { 'retry-after': '0' });
		}
		const wrong = this.#wrongEtags > 0;
		if (wrong) this.#wrongEtags--;
		const bytes = this.#snapshot;
		const seq = this.#seq.get(library[1]) ?? 0;
		const rebuilt = this.#rebuilds.get(library[1]);
		if (rebuilt !== undefined) {
			this.#rebuilds.delete(library[1]);
			this.#seq.set(library[1], rebuilt);
		}
		// In pieces, as a download arrives.
		let at = 0;
		const stall = this.#bodyHeld;
		const body = new ReadableStream<Uint8Array>({
			async pull(controller) {
				if (at >= bytes.length) return controller.close();
				if (stall && at > 0 && !stall.reached) {
					stall.reached = true;
					await new Promise<void>((resolve) => (stall.release = resolve));
				}
				controller.enqueue(bytes.slice(at, (at += 16384)));
			}
		});
		return new Response(body, {
			headers: {
				etag: `"${wrong ? '0'.repeat(64) : this.#etag}"`,
				'x-jewelcase-seq': String(seq),
				'x-jewelcase-schema': String(this.#schema ?? schemaVersion()),
				// A stale snapshot is asked for again at once rather than after the sync's wait.
				'retry-after': '0',
				'content-type': 'application/vnd.sqlite3'
			}
		});
	}

	/** A page of `feed` after the `after` of the request, or `410` for a position past its head. */
	#page(feed: Feed, url: URL): Response {
		const after = Number(url.searchParams.get('after'));
		const limit = Math.min(Number(url.searchParams.get('limit')), this.#pageSize);
		if (after > feed.head) return problem(410);
		const later = feed.changes.filter((change) => change.seq > after);
		const changes = later.slice(0, limit);
		const next = changes.length === later.length ? feed.head : changes[changes.length - 1].seq;
		return Response.json({ changes, next, head: feed.head });
	}
}
