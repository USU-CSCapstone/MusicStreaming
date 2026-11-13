// A dedicated worker for the catalog sync's tests, since OPFS file handles exist only in one:
// it runs `CatalogSync` against a `FakeApi` and does what a test asks of both, by name, with
// `{ id, path, args }` messages answered with `{ id, result }` or `{ id, error }`.

import init, {
	Catalog,
	Import,
	storeNames,
	storeRemove
} from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';
import { CatalogSync, type Report } from './sync';
import type { Kind } from './kinds';
import type { LibraryState } from './protocol';
import { FakeApi } from './fake-api';

/** What a test can ask of the worker. */
export class Harness {
	api = new FakeApi();
	private reports: Report[] = [];
	private changed: string[] = [];
	private kinds: (readonly Kind[])[] = [];
	private told: Record<string, LibraryState>[] = [];
	private heavies: boolean[] = [];
	/** The `limit` of each `commit` since the last reset. */
	private commitLimits: number[] = [];
	/**
	 * What a test asked the catalog's commits do: how long they seem to take (each none, and the
	 * one after `slowAfter` more `slowCommit` ms, or their real time while unset).
	 */
	private slowCommit?: number;
	private slowAfter = 0;
	#sync: CatalogSync | undefined;

	/** Closes any sync, deletes every file, and clears the fake server, with the snapshot to serve. */
	async reset(snapshot: Uint8Array) {
		await this.#sync?.close();
		for (const name of await storeNames()) await storeRemove(name);
		this.api = new FakeApi();
		await this.api.setSnapshot(snapshot);
		this.reports = [];
		this.changed = [];
		this.kinds = [];
		this.told = [];
		this.heavies = [];
		this.commitLimits = [];
		this.slowCommit = undefined;
		this.slowAfter = 0;
	}

	/** Starts a new sync for `user` as a restarted worker does, closing the old one. */
	async start(user = '1') {
		await this.#sync?.close();
		this.#sync = new CatalogSync({
			api: '/api/v1',
			user,
			fetch: this.api.fetch,
			report: (report) => this.reports.push(report),
			changed: (library, kinds) => {
				this.changed.push(library);
				this.kinds.push(kinds);
			},
			state: (libraries) => this.told.push(libraries),
			heavy: (heavy) => this.heavies.push(heavy)
		});
		await this.#sync.start();
	}

	/** What the sync has reported. */
	reported() {
		return this.reports;
	}

	/** The libraries the sync has announced as changed, in order. */
	changes() {
		return this.changed;
	}

	/** The kinds of each announcement, in the order of `changes`. */
	changedKinds() {
		return this.kinds;
	}

	/** What the sync has told the page each library stands at, each time it changed. */
	states() {
		return this.told;
	}

	/** The times the sync said it entered (true) and left (false) a call that holds the worker. */
	heavy() {
		return this.heavies;
	}

	/** Closes the sync, leaving the files. */
	async stop() {
		await this.#sync?.close();
	}

	/** Resolves once the sync has no pull under way. */
	idle() {
		return this.#sync?.idle();
	}

	/** Gives the sync a realtime message. */
	message(message: object) {
		this.#sync?.message(message as never);
	}

	/** Closes the sync and releases the pool to the next worker, which this one must not outlive. */
	async pause() {
		await this.#sync?.pause();
	}

	/** The connection settings the library's open file runs with, as the catalog reads them back. */
	pragmas(library: string) {
		return settings(this.#sync!.catalogs.get(library)!);
	}

	/**
	 * Opens `file` as another owner of the pool would, and says what it finds: its version and feed
	 * position.
	 */
	async inspect(file: string) {
		const catalog = await Catalog.open(file);
		const found = { version: catalog.schemaVersion(), position: Number(catalog.cursor('library')) };
		catalog.free();
		return found;
	}

	/** Signs the sync out, deleting every file. */
	async signOut() {
		await this.#sync?.signOut();
	}

	/** Whether a snapshot is being downloaded or installed. */
	busy() {
		return this.#sync!.busy();
	}

	/**
	 * Whether the library's file answers reads now, as the page's reads ask, rather than the server.
	 */
	onDevice(library: string) {
		return this.#sync!.onDevice(library);
	}

	/** The IDs of the libraries on the device. */
	libraries() {
		return [...(this.#sync?.catalogs.keys() ?? [])].sort();
	}

	/** The answer to a query of the library's file, as a person's page would ask it. */
	async query(library: string, name: string, params: object, offline = false) {
		return JSON.parse(await this.#sync!.query(library, name, params, offline));
	}

	/** The `limit` each `commit` was given, in order. */
	commits() {
		return this.commitLimits;
	}

	/**
	 * Called by the catalog's `commit`: notes `limit`, and gives the ms the commit should seem to
	 * take, or undefined for its real time.
	 */
	beforeCommit(limit: number) {
		this.commitLimits.push(limit);
		if (this.slowCommit === undefined) return undefined;
		return this.slowAfter-- === 0 ? this.slowCommit : 0;
	}

	/**
	 * Makes the `commit` after `calls` more seem to take `ms`, as on a slow device, and every other
	 * none, whatever the machine running the test takes; and forgets the limits so far.
	 */
	slowCommitAfter(calls: number, ms: number) {
		this.slowAfter = calls;
		this.slowCommit = ms;
		this.commitLimits = [];
	}

	/**
	 * Makes the next commit of any file fail with `message`, as a full device does, or the one
	 * after `after` more succeed, as one that fills up part-way through an apply does.
	 */
	failNextCommit(message: string, after = 0) {
		const commit = Catalog.prototype.commit;
		let left = after;
		Catalog.prototype.commit = function (...args: Parameters<typeof commit>) {
			if (left-- > 0) return commit.apply(this, args);
			Catalog.prototype.commit = commit;
			throw new Error(message);
		};
	}

	/**
	 * Makes the next write into a snapshot import fail with `message`, as a device that fills up
	 * part-way does.
	 */
	failNextImportWrite(message: string) {
		const write = Import.prototype.write;
		Import.prototype.write = function () {
			Import.prototype.write = write;
			throw new Error(message);
		};
	}

	/** Resolves once the library's file is part-way through an apply: its mark is set. */
	async untilMidApply(library: string) {
		const applying = () => this.#sync!.catalogs.get(library)?.applying('library') !== undefined;
		while (!applying()) await new Promise((resolve) => setTimeout(resolve));
	}

	/** Closes the sync once the library's file is part-way through an apply, as a closed tab does. */
	async stopMidApply(library: string) {
		await this.untilMidApply(library);
		await this.#sync!.close();
	}

	/**
	 * Waits, in the worker, until the library's file is part-way through an apply, and says what
	 * a read of the file itself finds then, where the file's mark says the apply goes, and what a
	 * person's read made then answers (or its error), online and offline.
	 */
	async midApply(library: string, name: string, params: object) {
		const sync = this.#sync!;
		await this.untilMidApply(library);
		const catalog = sync.catalogs.get(library)!;
		const settle = (read: Promise<string>) =>
			read.then(JSON.parse, (error: Error) => ({ error: error.message }));
		const online = settle(sync.query(library, name, params));
		const offline = settle(sync.query(library, name, params, true));
		return {
			partial: JSON.parse(catalog.query('1', library, name, JSON.stringify(params))),
			through: Number(catalog.applying('library')),
			online: await online,
			offline: await offline
		};
	}

	/**
	 * Waits until `applying` is part-way through an apply, then reads `library`, and says how many
	 * commits the apply made before the read was answered, and whether it was still part-way.
	 */
	async readDuringApply(applying: string, library: string, name: string, params: object) {
		const sync = this.#sync!;
		await this.untilMidApply(applying);
		const before = this.commitLimits.length;
		await sync.query(library, name, params);
		return {
			commits: this.commitLimits.length - before,
			applying: sync.catalogs.get(applying)!.applying('library') !== undefined
		};
	}

	/** A query of `library`'s file for the library `as`, whose rows the file should not hold. */
	queryAs(library: string, as: string, name: string, params: object) {
		return JSON.parse(
			this.#sync!.catalogs.get(library)!.query('1', as, name, JSON.stringify(params))
		);
	}

	/** A feed position the library's file holds. */
	cursor(library: string, feed: string) {
		return Number(this.#sync!.catalogs.get(library)!.cursor(feed));
	}

	/** The position the library's next pull of `feed` resumes after. */
	pulled(library: string, feed: string) {
		return Number(this.#sync!.catalogs.get(library)!.pulled(feed));
	}

	/** The names of the files on the device. */
	files() {
		return storeNames();
	}

	/** Deletes a file from under a stopped sync. */
	async remove(file: string) {
		await storeRemove(file);
	}

	/** Runs `sql` on a file from under a stopped sync. */
	async run(file: string, statements: string[]) {
		const catalog = await Catalog.open(file);
		for (const sql of statements) catalog.sql(sql);
		catalog.free();
	}

	/** Leaves a download file at the OPFS root, as a worker closed mid-download does. */
	async leaveDownload(name: string) {
		const root = await navigator.storage.getDirectory();
		await root.getFileHandle(name, { create: true });
	}

	/** The names of the download files at the OPFS root. */
	async downloads() {
		const root = await navigator.storage.getDirectory();
		const names: string[] = [];
		for await (const name of root.keys()) if (name.startsWith('download-')) names.push(name);
		return names;
	}

	/** Breaks the schema of the library's open file, so its next query finds it malformed. */
	damage(library: string) {
		const catalog = this.#sync!.catalogs.get(library)!;
		catalog.sql(
			"PRAGMA writable_schema = ON; UPDATE sqlite_schema SET sql = 'CREATE TABLE tracks (' WHERE name = 'tracks'; PRAGMA schema_version = 4242; PRAGMA writable_schema = OFF"
		);
	}
}

/** The connection settings a catalog's `ready` sets, as the file runs with them. */
const settings = (catalog: Catalog) =>
	Object.fromEntries(
		[
			'journal_mode',
			'locking_mode',
			'journal_size_limit',
			'cache_size',
			'temp_store',
			'synchronous'
		].map((name) => [name, catalog.pragma(name)])
	);

const harness = new Harness();

// The worker's clock, set back by what each commit a test timed took beyond the time it was to
// seem to take, so the sizes of slices do not hang on the machine running the test.
const now = performance.now.bind(performance);
let behind = 0;
performance.now = () => now() - behind;

// The harness records the catalog's commits, and a test can time them.
const { commit } = Catalog.prototype;
Catalog.prototype.commit = function (feed, through, limit) {
	const seems = harness.beforeCommit(limit);
	const started = now();
	try {
		return commit.call(this, feed, through, limit);
	} finally {
		if (seems !== undefined) behind += now() - started - seems;
	}
};
const ready = init();

/** Calls the harness method named by `path` with `args`, and answers with its result or error. */
onmessage = async ({ data: { id, path, args } }) => {
	try {
		await ready;
		let target: unknown = harness;
		let owner: unknown = harness;
		for (const key of path.split('.')) {
			owner = target;
			target = (target as Record<string, unknown>)[key];
		}
		const result = await (target as (...a: unknown[]) => unknown).apply(owner, args);
		postMessage({ id, result });
	} catch (error) {
		postMessage({ id, error: error instanceof Error ? error.message : String(error) });
	}
};
