// Manual scans for the mock, as the server's `/admin/libraries/{libraryId}/scans` answers them.
// The mock never writes the library's database (`requirements/general.md` §3.2), so a scan here
// is simulated: it walks the library's folders over a few seconds and finds nothing changed.
// It keeps the server's rules: one scan runs at a time, a scan queued and not yet started is the
// answer to another request, and a request while one runs queues one to follow it.

import type { Scan } from '../src/lib/api/types.ts';
import type { AdminResponse } from './plugins.ts';

/** What the simulator needs of a library: `null` from `files` when there is no such library. */
export type ScanSource = {
	/** How many files a scan of it meets. */
	files: (lib: string) => number | null;
	/** Its folders, relative to its root, in the order a scan walks them. */
	folders?: (lib: string) => string[];
	/** Its last scan that ran, before any here. */
	seed?: (lib: string) => Scan | undefined;
	/** The time, in milliseconds; tests move it on by hand. */
	now?: () => number;
};

/** A scan of this many files or fewer takes the shortest time, so its progress can be seen. */
const SHORTEST_MS = 3000;
const FILES_PER_SECOND = 500;

type Entry = {
	scan: Scan;
	queuedAt: number;
	/** When it started running, once it has. */
	startedAt?: number;
	files: number;
	folders: string[];
};

export class ScanSimulator {
	#libraries = new Map<string, Entry[]>();
	#lastId = 0;

	constructor(private readonly source: ScanSource) {}

	#now(): number {
		return (this.source.now ?? Date.now)();
	}

	/** The library's scans, oldest first, with the seeded one if it has one. */
	#entries(lib: string): Entry[] {
		let entries = this.#libraries.get(lib);
		if (!entries) {
			const seeded = this.source.seed?.(lib);
			entries = seeded ? [{ scan: seeded, queuedAt: 0, files: 0, folders: [] }] : [];
			this.#libraries.set(lib, entries);
		}
		return entries;
	}

	/** Brings the library's scans up to now: each starts once the one before it has finished. */
	#advance(lib: string): Entry[] {
		const now = this.#now();
		const entries = this.#entries(lib);
		let free = -Infinity;
		for (const entry of entries) {
			if (entry.scan.state === 'queued' && now >= Math.max(entry.queuedAt, free)) {
				entry.startedAt = Math.max(entry.queuedAt, free);
				entry.scan.state = 'running';
				entry.scan.startedAt = new Date(entry.startedAt).toISOString();
			}
			if (entry.scan.state !== 'running' || entry.startedAt === undefined) continue;
			const duration = Math.max(SHORTEST_MS, (entry.files / FILES_PER_SECOND) * 1000);
			const done = Math.min(1, (now - entry.startedAt) / duration);
			const { scan, files, folders } = entry;
			// The walk meets files a little ahead of reading them.
			scan.filesSeen = Math.round(files * Math.min(1, done * 1.25));
			scan.filesProcessed = Math.round(files * done);
			if (done < 1) {
				scan.currentFolder = folders[Math.floor(done * folders.length)] ?? '';
				free = Infinity;
			} else {
				scan.state = 'completed';
				scan.currentFolder = null;
				free = entry.startedAt + duration;
				scan.finishedAt = new Date(free).toISOString();
			}
		}
		return entries;
	}

	/** Queues a scan of the whole library, or answers the one already queued; `null` for no library. */
	start(lib: string): Scan | null {
		const files = this.source.files(lib);
		if (files === null) return null;
		const waiting = this.#advance(lib).find(
			(e) => e.scan.state === 'queued' && e.scan.wholeLibrary
		);
		if (waiting) return { ...waiting.scan };
		const entry: Entry = {
			scan: {
				id: String(++this.#lastId),
				trigger: 'manual',
				state: 'queued',
				startedAt: null,
				finishedAt: null,
				filesSeen: 0,
				filesProcessed: 0,
				added: 0,
				updated: 0,
				missing: 0,
				problems: 0,
				wholeLibrary: true,
				currentFolder: null
			},
			queuedAt: this.#now(),
			files,
			folders: this.source.folders?.(lib) ?? []
		};
		this.#entries(lib).push(entry);
		this.#advance(lib);
		return { ...entry.scan };
	}

	/**
	 * As `adminListScans`: queued and running scans and the last to finish, newest first. The
	 * last to finish is the last that ran, so a cancelled scan is never it.
	 */
	list(lib: string): Scan[] | null {
		if (this.source.files(lib) === null) return null;
		const entries = this.#advance(lib);
		const finished = entries
			.filter((e) => e.scan.finishedAt !== null && e.scan.state !== 'cancelled')
			.at(-1);
		return entries
			.filter((e) => e === finished || e.scan.state === 'queued' || e.scan.state === 'running')
			.reverse()
			.map((e) => ({ ...e.scan }));
	}
}

/**
 * `/admin/libraries/{libraryId}/scans` for any transport: the Vite middleware and the e2e
 * fixtures both call this, so tests exercise the same logic the dev server runs.
 */
export function scanRoute(
	simulator: ScanSimulator,
	method: string,
	path: string
): AdminResponse | undefined {
	const m = /^\/admin\/libraries\/([^/]+)\/scans$/.exec(path);
	if (!m) return undefined;
	const lib = decodeURIComponent(m[1]);
	const notFound = { status: 404, body: problem(404, 'Not Found', 'not_found') };
	if (method === 'GET') {
		const items = simulator.list(lib);
		return items ? { status: 200, body: { items } } : notFound;
	}
	if (method === 'POST') {
		const scan = simulator.start(lib);
		return scan ? { status: 202, body: scan } : notFound;
	}
	return { status: 405, body: problem(405, 'Method Not Allowed', 'method_not_allowed') };
}

function problem(status: number, title: string, code: string) {
	return { type: 'about:blank', title, status, code };
}
