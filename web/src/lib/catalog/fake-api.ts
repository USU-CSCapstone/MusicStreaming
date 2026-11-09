// A fake server for the catalog sync's tests: the library list, each library's gzipped snapshot and
// change feed, and the account feed, held in memory and served as `fetch` does. A test sets
// what the server holds and how it misbehaves, and reads what was asked.

import { schemaVersion } from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';

/** One entry of a change feed. */
type Change = {
	position: number;
	op: string;
	kind: string;
	id: string;
	key?: number;
	rows?: object;
};
/** A change feed: its entries in order and the position of the latest. */
type Feed = { changes: Change[]; head: number };

/** The bytes of a snapshot's body that arrive at a time. */
const PIECE = 1024;

/** The time to wait for a held response, so a test that forgets to release one fails. */
const HOLD_MS = 10_000;

/** A JSON error as the API sends one. */
const problem = (status: number, headers: Record<string, string> = {}) =>
	new Response(JSON.stringify({ code: String(status) }), {
		status,
		headers: { 'content-type': 'application/problem+json', ...headers }
	});

/** The fake server; `fetch` is its only way in, and the setters shape what it holds. */
export class FakeApi {
	/** Every path asked, with its query, in order. */
	requests: string[] = [];

	#libraries: string[] = [];
	#session = '1';
	#snapshot: Uint8Array = new Uint8Array(0);
	#position = new Map<string, number>();
	#rebuilds = new Map<string, number>();
	#schema: number | undefined;
	#etag = '';
	#feeds = new Map<string, Feed>();
	#account: Feed = { changes: [], head: 0 };
	#pageSize = 1000;
	/** The server's run, and the runs it can go on from. */
	#epoch = '1';
	#epochs = new Set(['1']);
	#failures: { match: string; after: number; status: number }[] = [];
	#building = 0;
	#wrongEtags = 0;
	#bodyHeld: { release: () => void; reached: boolean } | undefined;
	#cuts = 0;
	#ignoreIfRange = false;
	#ranges: string[] = [];
	#sent = 0;
	#held: { match: string; release: () => void; reached: boolean } | undefined;

	/** Serves the database `bytes`, gzipped, as every library's snapshot. */
	async setSnapshot(bytes: Uint8Array) {
		const gzip = new Blob([bytes as BlobPart]).stream().pipeThrough(new CompressionStream('gzip'));
		this.#snapshot = new Uint8Array(await new Response(gzip).arrayBuffer());
		const hash = await crypto.subtle.digest('SHA-256', this.#snapshot as BufferSource);
		this.#etag = [...new Uint8Array(hash)].map((b) => b.toString(16).padStart(2, '0')).join('');
	}

	/** The gzipped snapshot served, and its ETag. */
	snapshot() {
		return { gzip: this.#snapshot, etag: this.#etag };
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
	setPosition(library: string, position: number) {
		this.#position.set(library, position);
	}

	/**
	 * Moves the snapshot of `library` on to `position` after the next time it is asked for, as the
	 * server serves a stale snapshot once more while that request rebuilds it.
	 */
	rebuild(library: string, position: number) {
		this.#rebuilds.set(library, position);
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

	/**
	 * Starts a new run `epoch`. A restart goes on from the positions of every earlier run; a
	 * `restored` backup knows none of the runs before, as if they had come after the backup.
	 */
	restart(epoch: string, restored = false) {
		if (restored) this.#epochs.clear();
		this.#epochs.add(epoch);
		this.#epoch = epoch;
	}

	/** Returns at most this many changes a page, whatever limit is asked. */
	setPageSize(size: number) {
		this.#pageSize = size;
	}

	/** Answers requests whose path contains `match` with `status`, once `after` have been answered. */
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

	/** Breaks off the next `times` snapshot bodies after their first piece, as a lost network does. */
	cutBody(times = 1) {
		this.#cuts = times;
	}

	/** Serves the range asked for whatever `If-Range` names, as a server that ignores it does. */
	ignoreIfRange() {
		this.#ignoreIfRange = true;
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

	/** The `Range` of each snapshot request, or `''` for one without, in order. */
	ranges() {
		return this.#ranges;
	}

	/** The bytes of snapshot bodies sent. */
	sent() {
		return this.#sent;
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
		const response = failure
			? problem(failure.status)
			: this.#answer(url, new Headers(init?.headers));
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

	/** The answer to a request that was not made to fail, with the request's `headers`. */
	#answer(url: URL, headers: Headers): Response {
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
		const position = this.#position.get(library[1]) ?? 0;
		const rebuilt = this.#rebuilds.get(library[1]);
		if (rebuilt !== undefined) {
			this.#rebuilds.delete(library[1]);
			this.#position.set(library[1], rebuilt);
		}
		const etag = `"${wrong ? '0'.repeat(64) : this.#etag}"`;
		const length = this.#snapshot.length;
		const answer = {
			etag,
			'x-jewelcase-position': String(position),
			'x-jewelcase-epoch': this.#epoch,
			'x-jewelcase-schema': String(this.#schema ?? schemaVersion()),
			// A stale snapshot is asked for again at once rather than after the sync's wait.
			'retry-after': '0',
			'content-type': 'application/gzip'
		};
		// A range of the file `If-Range` names, as the server serves one.
		const range = headers.get('range') ?? '';
		this.#ranges.push(range);
		const start = Number(/^bytes=(\d+)-$/.exec(range)?.[1] ?? 0);
		const ranged =
			start > 0 && (this.#ignoreIfRange || [null, etag].includes(headers.get('if-range')));
		if (ranged && start >= length) {
			return new Response(null, {
				status: 416,
				headers: { ...answer, 'content-range': `bytes */${length}` }
			});
		}
		const bytes = this.#snapshot.subarray(ranged ? start : 0);
		// In pieces, as a download arrives.
		let at = 0;
		const stall = this.#bodyHeld;
		const cut = this.#cuts > 0;
		if (cut) this.#cuts--;
		const body = new ReadableStream<Uint8Array>({
			pull: async (controller) => {
				if (at >= bytes.length) return controller.close();
				if (cut && at > 0) return controller.error(new TypeError('network error'));
				if (stall && at > 0 && !stall.reached) {
					stall.reached = true;
					await new Promise<void>((resolve) => (stall.release = resolve));
				}
				const piece = bytes.slice(at, (at += PIECE));
				this.#sent += piece.length;
				controller.enqueue(piece);
			}
		});
		if (!ranged) return new Response(body, { headers: answer });
		const parted = { ...answer, 'content-range': `bytes ${start}-${length - 1}/${length}` };
		return new Response(body, { status: 206, headers: parted });
	}

	/**
	 * A page of `feed` after the `after` of the request, or `410` for a position past its head or
	 * of a run the server does not know.
	 */
	#page(feed: Feed, url: URL): Response {
		const after = Number(url.searchParams.get('after'));
		const limit = Math.min(Number(url.searchParams.get('limit')), this.#pageSize);
		if (after > feed.head) return problem(410);
		if (after > 0 && !this.#epochs.has(url.searchParams.get('epoch') ?? '')) return problem(410);
		const later = feed.changes.filter((change) => change.position > after);
		const changes = later.slice(0, limit);
		const next = changes.length === later.length ? feed.head : changes[changes.length - 1].position;
		return Response.json({ changes, next, head: feed.head, epoch: this.#epoch });
	}
}
