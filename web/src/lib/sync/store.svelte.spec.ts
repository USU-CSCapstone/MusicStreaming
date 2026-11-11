import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayReport } from '$lib/api/types';
import { connectivity } from '$lib/connectivity.svelte';
import type { Device } from './device';
import type { Local, Mutation } from './mutation';
import { Sender } from './sender';
import { CALL_MS, IdbPort, type Stored, browserEnv, wipeChannel, wipeDatabases } from './store';

// The real IndexedDB and Web Locks of the browser the specs run in.

let user: string;
/** A record owned by `tab`, with revision `rev`. */
const record = (key: string, rev = 'r1'): Stored => ({
	key,
	rev,
	owner: 'tab',
	createdAt: 1,
	tries: 0
});
/** The names of the databases the browser holds. */
const names = async () => (await indexedDB.databases()).map((db) => db.name);
/** The name of the current user's database. */
const database = () => `jewelcase-outbox-${user}`;
/** A device that makes nothing, as one with no library on it. */
/** What the device was asked to make. */
let made: Local[] = [];
/** A device that records what it is asked to make, and makes nothing. */
const device: Device = {
	// What it is asked to make, not what it is told was answered.
	apply: async (_user, local, answered) => void (answered || made.push(local)),
	rebase: async () => {},
	confirmed: async () => {}
};
/** A sender for the current user on the browser's database. */
function opened(fetcher?: typeof fetch) {
	const sender = new Sender();
	sender.start(user, { env: browserEnv(), fetch: fetcher, device });
	return sender;
}

beforeEach(() => {
	user = `spec-${crypto.randomUUID()}`;
	made = [];
});

afterEach(async () => {
	vi.useRealTimers();
	vi.restoreAllMocks();
	connectivity.setWorkOffline(false);
	await wipeDatabases([user]);
});

describe('the IndexedDB port', () => {
	it('keeps records, deletes by revision, and hands an orphan to one claimant', async () => {
		const port = new IdbPort(user);
		await port.put('mutations', record('a'));
		await port.put('mutations', record('b', 'r2'));
		expect((await port.all('mutations')).map((r) => r.key).sort()).toEqual(['a', 'b']);

		// A newer write is not deleted by the answer to the one it replaced.
		await port.remove('mutations', 'b', 'old');
		expect(await port.all('mutations')).toHaveLength(2);
		await port.remove('mutations', 'b', 'r2');
		await port.remove('mutations', 'a');
		expect(await port.all('mutations')).toEqual([]);

		await port.put('mutations', record('m'));
		expect(await port.claim('mutations', 'm', 'tab', 'first')).toMatchObject({ owner: 'first' });
		expect(await port.claim('mutations', 'm', 'tab', 'second')).toBeNull();
		expect((await port.all<Stored>('mutations'))[0].owner).toBe('first');
		port.close();
	});

	it('has a store for the log, the dead letters and the queue, and nothing is lost by reopening', async () => {
		const port = new IdbPort(user);
		for (const store of ['mutations', 'dead', 'queue'] as const)
			await port.put(store, record(store));
		port.close();
		const again = new IdbPort(user);
		expect((await again.all('dead')).map((r) => r.key)).toEqual(['dead']);
		again.close();
	});

	it('moves what the lanes of a version 2 database hold into the log, and sends it', async () => {
		// A database as version 2 made it: the lanes, the dead letters and the queue's store.
		const stamp = { rev: 'r', owner: 'closed-tab', createdAt: 1000, tries: 2 };
		const kept = (store: string): Stored[] => {
			if (store === 'plays') return [{ key: 'p1', ...stamp, report: { playId: 'p1' } } as Stored];
			if (store === 'marks')
				return [
					{
						key: '1/5',
						...stamp,
						library: '1',
						track: '5',
						liked: true,
						hidden: false,
						what: 'like “Signal”'
					} as Stored
				];
			if (store === 'playlists')
				return [
					{
						key: 'c1',
						...stamp,
						libraryId: '1',
						playlistId: '9',
						version: 4,
						fingerprint: '[]',
						verified: true,
						sent: 1,
						edits: [
							{ key: 'k1', change: { title: 'Sent' }, added: [] },
							{ key: 'k2', change: { title: 'Two' }, added: [] },
							{ key: 'k3', change: { pinned: true }, added: [] }
						]
					} as Stored
				];
			if (store === 'queue')
				return [{ key: 'instance', ...stamp, pending: [], starting: null } as Stored];
			if (store === 'dead')
				return [
					{
						key: 'marks/1/6',
						...stamp,
						library: '1',
						track: '6',
						liked: false,
						hidden: true,
						what: 'hide “Echo”'
					} as Stored
				];
			return [];
		};
		await new Promise<void>((resolve, reject) => {
			const request = indexedDB.open(database(), 2);
			request.onupgradeneeded = () => {
				for (const store of ['plays', 'marks', 'playlists', 'dead', 'queue']) {
					const made = request.result.createObjectStore(store, { keyPath: 'key' });
					for (const rec of kept(store)) made.put(rec);
				}
			};
			request.onsuccess = () => (request.result.close(), resolve());
			request.onerror = () => reject(request.error);
		});

		const port = new IdbPort(user);
		const log = (await port.all<Mutation>('mutations')).sort((a, b) => a.key.localeCompare(b.key));
		expect(
			log.map(({ name, scope, idem, tries, owner }) => ({ name, scope, idem, tries, owner }))
		).toEqual([
			{ name: 'mark', scope: 'mark:1/5', idem: 'r', tries: 2, owner: 'closed-tab' },
			// The first edit was confirmed; the two after it keep their keys and go in order.
			{ name: 'editPlaylist', scope: 'playlist:9', idem: 'k2', tries: 2, owner: 'closed-tab' },
			{ name: 'editPlaylist', scope: 'playlist:9', idem: 'k3', tries: 2, owner: 'closed-tab' },
			{ name: 'play', scope: 'plays', idem: 'r', tries: 2, owner: 'closed-tab' }
		]);
		expect(log[1].seq).toBeLessThan(log[2].seq);
		expect(log.slice(1, 3).map((m) => (m.args as { baseVersion: number }).baseVersion)).toEqual([
			4, 5
		]);
		expect((await port.all('queue')).map((r) => r.key)).toEqual(['instance']);
		// What it set aside is a mutation set aside, which can be retried or discarded.
		const [aside, ...more] = await port.all<Mutation>('dead');
		expect(more).toEqual([]);
		expect(aside).toMatchObject({ key: 'marks/1/6', name: 'mark', scope: 'mark:1/6' });
		const db = await new Promise<IDBDatabase>((resolve) => {
			const request = indexedDB.open(database());
			request.onsuccess = () => resolve(request.result);
		});
		expect([...db.objectStoreNames].sort()).toEqual(['dead', 'mutations', 'queue']);
		db.close();
		port.close();

		const sent: string[] = [];
		const fetcher = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
			const path = new URL(String(input), 'http://test').pathname.replace(/^.*\/api\/v1/, '');
			sent.push(`${init.method ?? 'GET'} ${path}`);
			if (init.method === 'PATCH') {
				const base = JSON.parse(String(init.body)).baseVersion;
				return Response.json({ id: '9', libraryId: '1', version: base + 1 });
			}
			return new Response(null, { status: 204 });
		}) as typeof fetch;
		const sender = opened(fetcher);
		await vi.waitFor(() => expect(sent).toHaveLength(4));
		// Version 2 made nothing on the device: it is asked to as the mutations are taken, each
		// write at the time its mutation was made.
		const calls = made.flatMap((local) => local.calls);
		expect(
			calls.map(({ name, at }) => ({ name, at })).sort((a, b) => a.name.localeCompare(b.name))
		).toEqual([
			{ name: 'editPlaylist', at: 1000 },
			{ name: 'editPlaylist', at: 1000 },
			{ name: 'like', at: 1000 }
		]);
		await vi.waitFor(async () => expect(await sender.pending()).toBe(0));
		sender.stop();
		expect(sent.sort()).toEqual([
			'PATCH /libraries/1/playlists/9',
			'PATCH /libraries/1/playlists/9',
			'POST /me/plays',
			'PUT /libraries/1/tracks/5/like'
		]);
	});

	it('closes its connection when another deletes the database, so nothing blocks, and never makes it again', async () => {
		const port = new IdbPort(user);
		await port.put('mutations', record('a'));
		const blocked = vi.fn();
		const request = indexedDB.deleteDatabase(database());
		request.onblocked = blocked;
		await new Promise((resolve) => (request.onsuccess = resolve));
		expect(blocked).not.toHaveBeenCalled();
		// The next call fails rather than make the deleted database again.
		await expect(port.put('mutations', record('b'))).rejects.toThrow();
		expect(await names()).not.toContain(database());
		port.close();
	});

	it('opens the connection again after the browser closed it under a call', async () => {
		const port = new IdbPort(user);
		await port.put('mutations', record('a'));
		for (const name of ['InvalidStateError', 'UnknownError']) {
			const transaction = vi
				.spyOn(IDBDatabase.prototype, 'transaction')
				.mockImplementationOnce(() => {
					throw new DOMException('The connection is closing.', name);
				});
			expect(
				(await port.all('mutations')).map((r) => r.key),
				name
			).toEqual(['a']);
			expect(transaction).toHaveBeenCalledTimes(2);
			transaction.mockRestore();
		}
		port.close();
	});

	it('gives up on a call that does not answer within the bound, and tries afresh the next time', async () => {
		vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
		const port = new IdbPort(user);
		const open = vi.spyOn(indexedDB, 'open').mockReturnValueOnce({} as IDBOpenDBRequest);
		const failure = port.all('mutations').then(
			() => null,
			(error: DOMException) => error.name
		);
		await vi.advanceTimersByTimeAsync(CALL_MS);
		expect(await failure).toBe('TimeoutError');
		expect(open).toHaveBeenCalledTimes(1);
		vi.useRealTimers();
		expect(await port.all('mutations')).toEqual([]);
		port.close();
	});

	it('does not fail on a database that is upgraded by another connection', async () => {
		const port = new IdbPort(user);
		await port.put('mutations', record('a'));
		const upgraded = await new Promise<IDBDatabase>((resolve, reject) => {
			const request = indexedDB.open(database(), 4);
			request.onupgradeneeded = () =>
				request.result.createObjectStore('queues', { keyPath: 'key' });
			request.onsuccess = () => resolve(request.result);
			request.onerror = request.onblocked = () => reject(request.error ?? new Error('blocked'));
		});
		expect([...upgraded.objectStoreNames]).toContain('mutations');
		upgraded.close();
		port.close();
	});
});

describe('wiping', () => {
	it('tells every connection to close, and deletes the database', async () => {
		const port = new IdbPort(user);
		await port.put('mutations', record('a'));
		expect(await names()).toContain(database());
		await wipeDatabases([user]);
		expect(await names()).not.toContain(database());
		port.close();
	});

	it('tells the connections again when one blocks the deletion, and gives up waiting after the bound', async () => {
		await new IdbPort(user).put('mutations', record('a'));
		// A connection that ignores the channel and the version change, as a stuck tab does.
		const stuck = await new Promise<IDBDatabase>((resolve) => {
			const request = indexedDB.open(database());
			request.onsuccess = () => resolve(request.result);
		});
		const told = vi.fn();
		const channel = wipeChannel(user)!;
		channel.onmessage = told;
		const started = Date.now();
		await wipeDatabases([user]);
		expect(Date.now() - started).toBeGreaterThanOrEqual(CALL_MS - 100);
		expect(told.mock.calls.length).toBeGreaterThanOrEqual(2);
		// The deletion waits behind the stuck connection and goes through once it lets go.
		stuck.close();
		await vi.waitFor(async () => expect(await names()).not.toContain(database()));
		channel.close();
	});

	it('makes an open sender let go and write nothing more', async () => {
		const sender = opened();
		connectivity.setWorkOffline(true);
		sender.enqueue('play', { playId: 'p1' } as PlayReport);
		await sender.idle();
		expect(await sender.pending()).toBe(1);

		await wipeDatabases([user]);
		sender.enqueue('play', { playId: 'p2' } as PlayReport);
		await sender.idle();
		expect(await names()).not.toContain(database());
		expect(await sender.pending()).toBe(0);
	});
});

describe('a sender in the browser', () => {
	it('keeps what was made across a close, and sends it from the next page once', async () => {
		const posted: string[] = [];
		const fetcher = (async (_input: RequestInfo | URL, init: RequestInit = {}) => {
			for (const item of JSON.parse(String(init.body)).items) posted.push(item.playId);
			return new Response(null, { status: 204 });
		}) as typeof fetch;
		connectivity.setWorkOffline(true);
		const before = opened(fetcher);
		before.enqueue('play', { playId: 'p1' } as PlayReport);
		before.enqueue('play', { playId: 'p2' } as PlayReport);
		await before.idle();
		before.stop();
		// Nothing is held in memory by the next page: only the database remembers.
		connectivity.setWorkOffline(false);
		const after = opened(fetcher);
		await after.idle();
		after.stop();
		expect(posted.sort()).toEqual(['p1', 'p2']);
	});
});
