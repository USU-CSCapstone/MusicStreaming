// A dedicated worker for the catalog sync's tests, since OPFS file handles exist only in one:
// it runs `CatalogSync` against a `FakeApi` and does what a test asks of both, by name, with
// `{ id, path, args }` messages answered with `{ id, result }` or `{ id, error }`.

import init, {
	Catalog,
	storeNames,
	storeRemove
} from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';
import { CatalogSync, type Report } from './sync';
import { FakeApi } from './fake-api';

/** What a test can ask of the worker. */
export class Harness {
	api = new FakeApi();
	private reports: Report[] = [];
	private changed: string[] = [];
	#sync: CatalogSync | undefined;

	/**
	 * Closes any sync, deletes every file and download, and clears the fake server, with the
	 * database to serve as the snapshot.
	 */
	async reset(snapshot: Uint8Array) {
		await this.#sync?.close();
		for (const name of await storeNames()) await storeRemove(name);
		const root = await navigator.storage.getDirectory();
		for (const name of await this.downloads()) await root.removeEntry(name);
		this.api = new FakeApi();
		await this.api.setSnapshot(snapshot);
		this.reports = [];
		this.changed = [];
	}

	/** Starts a new sync for `user` as a restarted worker does, closing the old one. */
	async start(user = '1') {
		await this.#sync?.close();
		this.#sync = new CatalogSync({
			api: '/api/v1',
			user,
			fetch: this.api.fetch,
			report: (report) => this.reports.push(report),
			changed: (library) => this.changed.push(library)
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

	/** Signs the sync out, deleting every file. */
	async signOut() {
		await this.#sync?.signOut();
	}

	/** Whether a snapshot is being downloaded or installed. */
	busy() {
		return this.#sync!.busy();
	}

	/** Whether the library's file answers reads now, as the page's reads ask, rather than the server. */
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

	/** Makes the next commit of any file fail with `message`, as a full device does. */
	failNextCommit(message: string) {
		const commit = Catalog.prototype.commit;
		Catalog.prototype.commit = function () {
			Catalog.prototype.commit = commit;
			throw new Error(message);
		};
	}

	/** Resolves once the library's file is part-way through an apply: its mark is set. */
	async #midApply(library: string) {
		const applying = () => this.#sync!.catalogs.get(library)?.applying('library') !== undefined;
		while (!applying()) await new Promise((resolve) => setTimeout(resolve));
	}

	/** Closes the sync once the library's file is part-way through an apply, as a closed tab does. */
	async stopMidApply(library: string) {
		await this.#midApply(library);
		await this.#sync!.close();
	}

	/**
	 * Waits, in the worker, until the library's file is part-way through an apply, and says what
	 * a read of the file itself finds then, where the file's mark says the apply goes, and what a
	 * person's read made then answers (or its error), online and offline.
	 */
	async midApply(library: string, name: string, params: object) {
		const sync = this.#sync!;
		await this.#midApply(library);
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

	/** The epoch the library's file holds for `feed`. */
	epoch(library: string, feed: string) {
		return this.#sync!.catalogs.get(library)!.epoch(feed);
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

	/** Leaves a download file holding `bytes` at the OPFS root, as a worker closed mid-download does. */
	async leaveDownload(name: string, bytes: Uint8Array = new Uint8Array(0)) {
		const root = await navigator.storage.getDirectory();
		const writable = await (await root.getFileHandle(name, { create: true })).createWritable();
		await writable.write(bytes as BlobPart);
		await writable.close();
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

const harness = new Harness();
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
