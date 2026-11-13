// Where the sync framework keeps what has not reached the server (`design/web.md`, The sync
// framework): one database for each signed-in user, behind a small port so the sender's rules are
// tested without a browser. IndexedDB is the port in browsers and memory the port in node specs.
//
// A store is a keyed set of records: `mutations`, the log; `dead`, the mutations set aside; and
// `queue`, the account's queue as the server last confirmed it, which the queue worker shows the
// log's unsent starts and edits on top of when the app opens. A new store joins as a further name in
// `STORES`; a raised `VERSION` creates it in `onupgradeneeded`. An upgrade from version 2 moves the
// records of the old lanes (`plays`, `marks`, `playlists`) into the log (`migrate.ts`).

import { type Lanes, migrate, setAsideOf } from './migrate';

/** The stores of a user's database. */
export const STORES = ['mutations', 'dead', 'queue'] as const;
/** The name of one of the stores. */
export type StoreName = (typeof STORES)[number];

/** What every record carries: its key in its store, and who wrote it and when. */
export type Stored = {
	key: string;
	/** Changes with every write, so a delete can tell that a newer write replaced the one it sent. */
	rev: string;
	/** The tab instance that sends it, until that tab is gone and another adopts it. */
	owner: string;
	createdAt: number;
	/** Answered failures so far. */
	tries: number;
};

/** A user's records. Every call is bounded, and fails rather than hangs. */
export interface Port {
	/** Every record in `store`. */
	all<T extends Stored>(store: StoreName): Promise<T[]>;
	/**
	 * Writes the record; with `strict`, it has reached the disk when this settles, which a write of a
	 * user's action wants.
	 */
	put(store: StoreName, record: Stored, strict?: boolean): Promise<void>;
	/** Deletes the record, but with `rev` only if it still has that revision: a newer write stays. */
	remove(store: StoreName, key: string, rev?: string): Promise<void>;
	/**
	 * Makes `to` the owner of the record if `from` still is, and returns it; null if another claimed
	 * it first.
	 */
	claim<T extends Stored>(
		store: StoreName,
		key: string,
		from: string,
		to: string
	): Promise<T | null>;
	/** Lets go of the connection; a later call opens it again. */
	close(): void;
}

/** Runs `run` while holding the lock `name`, unless another holds it: `ran` says which. */
export type TryLock = <T>(
	name: string,
	run: () => Promise<T>
) => Promise<{ ran: true; value: T } | { ran: false }>;

/** What the sender and the queue worker run on. */
export type Env = { open(user: string): Port; tryLock: TryLock };

/**
 * The start of every user's database name, and of the lock under which closed tabs' mutations are
 * adopted. The name is that of the version 2 database, so an upgrade finds its records.
 */
const PREFIX = 'jewelcase-outbox-';
/** The database of `user`; also the name of the channel that tells its connections to close. */
const databaseName = (user: string) => `${PREFIX}${user}`;
/** The database's schema version; raised when a store is added to `STORES`. */
const VERSION = 3;
/** The stores of version 2, whose records version 3 moves into the log. */
const LANES = ['plays', 'marks', 'playlists'] as const;
/** The longest any database call is waited for. */
export const CALL_MS = 2000;

/**
 * Memory: records kept as long as the environment is, shared by every port opened on it, as a
 * database is.
 */
export function memoryEnv(): Env {
	const dbs = new Map<string, Map<StoreName, Map<string, Stored>>>();
	const held = new Set<string>();
	return {
		open(user) {
			const db = dbs.get(user) ?? new Map(STORES.map((store) => [store, new Map()]));
			dbs.set(user, db);
			return {
				all: async <T extends Stored>(store: StoreName) =>
					[...db.get(store)!.values()].map((record) => structuredClone(record) as T),
				put: async (store, record) => void db.get(store)!.set(record.key, structuredClone(record)),
				remove: async (store, key, rev) => {
					if (rev === undefined || db.get(store)!.get(key)?.rev === rev) db.get(store)!.delete(key);
				},
				claim: async <T extends Stored>(
					store: StoreName,
					key: string,
					from: string,
					to: string
				) => {
					const record = db.get(store)!.get(key);
					if (record?.owner !== from) return null;
					db.get(store)!.set(key, { ...record, owner: to });
					return structuredClone({ ...record, owner: to }) as T;
				},
				close() {}
			};
		},
		async tryLock(name, run) {
			if (held.has(name)) return { ran: false };
			held.add(name);
			try {
				return { ran: true, value: await run() };
			} finally {
				held.delete(name);
			}
		}
	};
}

/** Whether this browser keeps IndexedDB and Web Locks. */
const available = () =>
	typeof indexedDB !== 'undefined' &&
	typeof navigator !== 'undefined' &&
	typeof navigator.locks !== 'undefined';

/**
 * The browser's: IndexedDB and Web Locks; memory where it has none, which keeps nothing past the
 * page.
 */
export function browserEnv(): Env {
	if (!available()) return fallback;
	return {
		open: (user) => new IdbPort(user),
		tryLock: (name, run) =>
			navigator.locks.request(name, { ifAvailable: true }, async (lock) =>
				lock ? { ran: true as const, value: await run() } : { ran: false as const }
			)
	};
}
/** The one memory environment every `browserEnv()` without IndexedDB or Web Locks shares. */
const fallback = memoryEnv();

/**
 * The lock an instance holds while it lives: with it free, the instance is closed and what it left
 * may be taken.
 */
const tabLock = (instance: string) => `jewelcase-tab-${instance}`;

/**
 * The lock under which one page's sender at a time takes over what closed tabs' senders left of
 * `user`'s.
 */
export const adoptLock = (user: string) => `${PREFIX}${user}`;

/** Marks `instance` as alive in `env` until the returned function is called. */
export function holdTab(env: Env, instance: string): () => void {
	let release!: () => void;
	const gate = new Promise<void>((resolve) => (release = resolve));
	void env.tryLock(tabLock(instance), () => gate);
	return release;
}

/**
 * Whether `instance` is closed: its lock is free, and is held no longer than it takes to see that.
 */
export async function closed(env: Env, instance: string): Promise<boolean> {
	return (await env.tryLock(tabLock(instance), async () => {})).ran;
}

/** Fails with a `TimeoutError` if `work` does not settle within `CALL_MS`. */
function bounded<T>(work: Promise<T>): Promise<T> {
	let timer: ReturnType<typeof setTimeout>;
	const late = new Promise<never>((_, reject) => {
		timer = setTimeout(
			() => reject(new DOMException('The database did not answer.', 'TimeoutError')),
			CALL_MS
		);
	});
	return Promise.race([work, late]).finally(() => clearTimeout(timer));
}

/** Errors a closed or broken connection throws, which a fresh connection cures. */
const stale = (error: unknown) =>
	error instanceof DOMException &&
	(error.name === 'UnknownError' || error.name === 'InvalidStateError');

/**
 * Moves what version 2's lanes hold into the log and deletes them, and makes what it set aside
 * mutations set aside, inside the upgrade's transaction `tx`, so the database is never seen with
 * the lanes' records in neither place.
 */
function upgrade(db: IDBDatabase, tx: IDBTransaction) {
	const lanes = LANES.filter((lane) => db.objectStoreNames.contains(lane));
	if (lanes.length === 0) return;
	const dead = tx.objectStore('dead');
	const aside = dead.getAll();
	aside.onsuccess = () => {
		for (const rec of aside.result) {
			const mutations = setAsideOf(rec);
			if (!mutations) continue;
			dead.delete(rec.key);
			for (const mutation of mutations) dead.put(mutation);
		}
	};
	const read: Partial<Lanes> = {};
	for (const lane of lanes) {
		const request = tx.objectStore(lane).getAll();
		request.onsuccess = () => {
			read[lane] = request.result;
			if (Object.keys(read).length < lanes.length) return;
			const log = tx.objectStore('mutations');
			for (const mutation of migrate(read)) log.put(mutation);
			for (const old of lanes) db.deleteObjectStore(old);
		};
	}
}

/** `user`'s database in IndexedDB. */
export class IdbPort implements Port {
	#name: string;
	#db: Promise<IDBDatabase> | null = null;
	/** The database is being deleted: it is never opened again, which would make it anew. */
	#deleted = false;

	/** A port on `user`'s database, which opens on first use. */
	constructor(user: string) {
		this.#name = databaseName(user);
	}

	/**
	 * The connection, opened once and closed whenever another connection wants to change or delete
	 * the database; once it is deleted, every call fails rather than make it again.
	 */
	#open(): Promise<IDBDatabase> {
		if (this.#deleted)
			return Promise.reject(new DOMException('The database was deleted.', 'AbortError'));
		return (this.#db ??= new Promise<IDBDatabase>((resolve, reject) => {
			const request = indexedDB.open(this.#name, VERSION);
			request.onupgradeneeded = () => {
				const db = request.result;
				for (const store of STORES)
					if (!db.objectStoreNames.contains(store)) db.createObjectStore(store, { keyPath: 'key' });
				upgrade(db, request.transaction!);
			};
			request.onsuccess = () => {
				const db = request.result;
				db.onversionchange = (event) => {
					if (event.newVersion === null) this.#deleted = true;
					this.close();
				};
				resolve(db);
			};
			request.onerror = () => reject(request.error);
		}).catch((error) => {
			this.#db = null;
			throw error;
		}));
	}

	/** Closes the connection; the next call opens it again. */
	close() {
		const db = this.#db;
		this.#db = null;
		db?.then((open) => open.close()).catch(() => {});
	}

	/**
	 * One transaction on `store`, resolving when it has committed. A connection that was closed
	 * under it, or broke, is opened again once; one that does not answer in time is dropped.
	 */
	async #run<T>(
		mode: IDBTransactionMode,
		store: StoreName,
		use: (store: IDBObjectStore) => IDBRequest<T>,
		strict = false
	): Promise<T> {
		for (let attempt = 0; ; attempt++) {
			try {
				const db = await bounded(this.#open());
				return await bounded(
					new Promise<T>((resolve, reject) => {
						const tx = db.transaction(store, mode, strict ? { durability: 'strict' } : undefined);
						const request = use(tx.objectStore(store));
						tx.oncomplete = () => resolve(request.result);
						tx.onabort = tx.onerror = () => reject(tx.error ?? request.error);
					})
				);
			} catch (error) {
				this.close();
				if (attempt === 0 && stale(error)) continue;
				throw error;
			}
		}
	}

	/** Every record in `store`. */
	all<T extends Stored>(store: StoreName) {
		return this.#run('readonly', store, (s) => s.getAll() as IDBRequest<T[]>);
	}

	/** Writes `record`; with `strict`, it is on disk when this settles. */
	async put(store: StoreName, record: Stored, strict = false) {
		await this.#run('readwrite', store, (s) => s.put(record), strict);
	}

	/** Hands the record to `to` in one transaction if `from` still owns it; null if not. */
	async claim<T extends Stored>(store: StoreName, key: string, from: string, to: string) {
		let claimed: T | null = null;
		await this.#run('readwrite', store, (s) => {
			const read = s.get(key);
			read.onsuccess = () => {
				if (read.result?.owner !== from) return;
				claimed = { ...read.result, owner: to };
				s.put(claimed);
			};
			return read;
		});
		return claimed;
	}

	/** Deletes the record, or with `rev` only if it still has that revision. */
	async remove(store: StoreName, key: string, rev?: string) {
		await this.#run('readwrite', store, (s) => {
			if (rev === undefined) return s.delete(key);
			const read = s.get(key);
			read.onsuccess = () => {
				if (read.result?.rev === rev) s.delete(key);
			};
			return read;
		});
	}
}

/**
 * Tells every tab's connection to `user`'s database that it is wiped, so each lets go and writes no
 * more.
 */
function tellToClose(user: string) {
	if (typeof BroadcastChannel === 'undefined') return;
	const channel = new BroadcastChannel(databaseName(user));
	channel.postMessage('wipe');
	channel.close();
}

/**
 * Deletes the database `name`, telling the tabs again if one holds it open; gives up waiting after
 * `CALL_MS`, when the deletion stays queued until they let go.
 */
function deleteDatabase(name: string, user: string): Promise<void> {
	return new Promise((resolve) => {
		const timer = setTimeout(resolve, CALL_MS);
		const request = indexedDB.deleteDatabase(name);
		request.onsuccess = request.onerror = () => {
			clearTimeout(timer);
			resolve();
		};
		request.onblocked = () => tellToClose(user);
	});
}

/**
 * Deletes the databases of `users` and of any other user this browser has one for, after telling
 * every tab to close its connection and stop writing. A deletion a tab blocks is queued behind
 * it, so nothing is read from a database that was to be wiped. The sync framework's `wipe`.
 */
export async function wipeDatabases(users: Iterable<string>) {
	if (typeof indexedDB === 'undefined') return;
	const names = new Set([...users].map(databaseName));
	try {
		for (const { name } of (await indexedDB.databases?.()) ?? [])
			if (name?.startsWith(PREFIX)) names.add(name);
	} catch {
		// A browser that cannot list databases wipes the ones it knows.
	}
	await Promise.all(
		[...names].map(async (name) => {
			const user = name.slice(PREFIX.length);
			tellToClose(user);
			// A tick for the tabs to hear it before the deletion asks them.
			await new Promise((resolve) => setTimeout(resolve));
			await deleteDatabase(name, user);
		})
	);
}

/** The channel on which `user`'s database hears that it is wiped. */
export const wipeChannel = (user: string) =>
	typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel(databaseName(user));

/** How many plays and changes `port` holds that the server has not confirmed: the log's mutations. */
export async function unsent(port: Port): Promise<number> {
	return (await port.all('mutations')).length;
}

/**
 * The key, in this tab's session storage, of how many unsent plays and changes a session ending
 * discarded: told once on the login page.
 */
export const DISCARDED = 'jewelcase-discarded';
