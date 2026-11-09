import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import snapshotUrl from '../../../../crates/catalog/parity/library.db?url';
import library from '../../../../crates/catalog/parity/library.json';
import searchCases from '../../../../crates/catalog/parity/search.json';
import recipeCases from '../../../../crates/catalog/parity/recipes.json';
import type { Harness } from './fake-worker';

/** What a test asks of the worker, each call answered when the worker is done. */
type Remote<T> = {
	[K in keyof T]: T[K] extends (...args: infer A) => infer R
		? (...args: A) => Promise<Awaited<R>>
		: Remote<T[K]>;
};

// OPFS file handles exist only in a dedicated worker, so the sync runs in one (`fake-worker.ts`),
// against a fake server, and the tests ask it what it did.
let worker: Worker;
let calls = 0;
const waiting = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();

/** Calls `path` of the worker's harness with `args`. */
function call(path: string, args: unknown[]) {
	return new Promise((resolve, reject) => {
		const id = ++calls;
		waiting.set(id, { resolve, reject });
		worker.postMessage({ id, path, args });
	});
}

/** A proxy whose calls go to the worker, by the path of names they were reached by. */
function remote<T>(path: string[] = []): Remote<T> {
	return new Proxy(() => {}, {
		get: (_, key) => remote([...path, String(key)]),
		apply: (_, __, args) => call(path.join('.'), args)
	}) as unknown as Remote<T>;
}

const h = remote<Harness>();
let snapshot: Uint8Array;

beforeAll(async () => {
	snapshot = new Uint8Array(await (await fetch(snapshotUrl)).arrayBuffer());
	worker = new Worker(new URL('./fake-worker.ts', import.meta.url), { type: 'module' });
	worker.onmessage = ({ data: { id, result, error } }) => {
		const { resolve, reject } = waiting.get(id)!;
		waiting.delete(id);
		if (error === undefined) resolve(result);
		else reject(new Error(error));
	};
});
afterAll(() => worker.terminate());
beforeEach(() => h.reset(snapshot));

/** The feed position the fake server's snapshots are current to. */
const SEQ = 10;

/** A tag's upsert at `seq`, as the library feed carries it. */
const tag = (seq: number, id: string, name: string) => ({
	seq,
	op: 'upsert',
	kind: 'tag',
	id,
	rows: {
		tags: [
			{
				id,
				library_id: '1',
				type: 'genre',
				name,
				sort_key: btoa(name),
				track_count: 0,
				album_count: 0,
				artist_count: 0
			}
		],
		tag_types: [{ key: 'genre', name: 'Genre' }]
	}
});

/** The account's playlists in the golden library, as the account feed carries them. */
const playlists = library
	.filter((rows) => rows.kind === 'playlist' || rows.kind === 'playlistItem')
	.map((rows, i) => ({
		seq: i + 1,
		op: 'upsert',
		kind: rows.kind,
		id: rows.id,
		key: rows.key,
		rows: rows.tables as unknown as Record<string, Record<string, unknown>[]>
	}));
/** The same account's playlist in another library, which this library's file must not hold. */
const elsewhere = {
	seq: playlists.length + 1,
	op: 'upsert',
	kind: 'playlist',
	id: '504',
	rows: {
		playlists: [{ ...playlists[0].rows.playlists[0], id: '504', library_id: '2' }]
	}
};
const accountHead = playlists.length + 1;

/** A server with libraries `ids`, each at `SEQ` with nothing newer. */
async function serve(...ids: string[]) {
	await h.api.setLibraries(ids);
	for (const id of ids) {
		await h.api.setSeq(id, SEQ);
		await h.api.setFeed(id, [], SEQ);
	}
	await h.api.setAccount([...playlists, elsewhere], accountHead);
}

/** Starts the worker's sync on the served libraries and waits for it to settle. */
async function started(...ids: string[]) {
	await serve(...ids);
	await h.start();
	await h.idle();
}

/** The feed position `grown` moves a library to. */
const BASE = SEQ + 1;

/**
 * Moves the started library on to `BASE`, a catch-up of one change that gives it `tracks` tracks,
 * so the catch-ups that follow are each well under a tenth of it.
 */
async function grown(tracks = 1000) {
	const counts = {
		seq: BASE,
		op: 'upsert',
		kind: 'library',
		id: '1',
		rows: { libraries: [{ id: '1', track_count: tracks, album_count: 0, artist_count: 0 }] }
	};
	await h.api.setFeed('1', [counts], BASE);
	await h.message(libraryChanged(BASE));
	await h.idle();
}

/** An album's upsert at `seq`, as the library feed carries it. */
const album = (seq: number, id: string, title: string) => ({
	seq,
	op: 'upsert',
	kind: 'album',
	id,
	rows: {
		albums: [
			{
				added_at: 5000,
				artist_sort_key: '',
				available_track_count: 0,
				disc_count: 1,
				disc_total: null,
				duration_us: 0,
				id,
				image_id: null,
				labels: '[]',
				library_id: '1',
				loudness_lufs: null,
				peak_dbtp: null,
				release_date: null,
				sort_key: btoa(title.toLowerCase()),
				title,
				track_count: 0,
				track_total: null,
				type: 'album'
			}
		]
	}
});

const hello = { type: 'hello', deviceId: '1', serverTime: '' };
/** The realtime hint that `libraryId`'s feed reached `seq`. */
const libraryChanged = (seq: number, libraryId = '1') => ({
	type: 'libraryChanged',
	libraryId,
	seq
});
/** The name the device's catalog holds for tag `id`, or undefined when it has none. */
const tagName = async (id: string) =>
	((await h.query('1', 'getTag', { tagId: id })) as { name: string } | null)?.name;
/** The total of the device's catalog answer to the list query `name`. */
const total = async (name: string) =>
	((await h.query('1', name, { limit: 1 })) as { total: number }).total;

/** The number of changes in a catch-up large enough to apply in many chunks. */
const LARGE = 2000;

/** The `LARGE` changes of a catch-up: new tags after `BASE`. */
const largeFeed = () =>
	Array.from({ length: LARGE }, (_, i) => tag(BASE + 1 + i, `${100 + i}`, `Tag ${i}`));

/** Starts library 1, large enough that `LARGE` changes are a catch-up, and serves them. */
async function largeCatchUp() {
	await started('1');
	await grown(100 * LARGE);
	const before = await total('listTags');
	await h.api.setFeed('1', largeFeed(), BASE + LARGE);
	await h.message(libraryChanged(BASE + LARGE));
	return before;
}

describe('catalog sync', () => {
	it('a library appears only after its snapshot is swapped in', async () => {
		await serve('1');
		await h.api.hold('/catalog');
		await h.start();
		await expect.poll(() => h.api.holding()).toBe(true);
		expect(await h.libraries()).toEqual([]);
		expect(await h.files()).toEqual([]);

		await h.api.release();
		await h.idle();
		expect(await h.libraries()).toEqual(['1']);
		expect(await total('listTracks')).toBe(6);
		expect([await h.cursor('1', 'library'), await h.cursor('1', 'account')]).toEqual([
			SEQ,
			accountHead
		]);
		expect(await h.reported()).toEqual([]);
	});

	it('catch-up after a hint fetches only newer changes and applies them at the head', async () => {
		await started('1');
		await grown();
		await h.api.setPageSize(1);
		await h.api.setFeed('1', [tag(BASE + 1, '90', 'Rock'), tag(BASE + 2, '91', 'Jazz')], BASE + 2);
		const asked = (await h.api.asked('/')).length;

		await h.message(libraryChanged(BASE + 2));
		await h.idle();
		expect((await h.api.asked('/')).slice(asked)).toEqual([
			`/libraries/1/changes?after=${BASE}&epoch=1&limit=1000`,
			`/libraries/1/changes?after=${BASE + 1}&epoch=1&limit=1000`
		]);
		expect([await tagName('90'), await tagName('91')]).toEqual(['Rock', 'Jazz']);
		expect(await h.cursor('1', 'library')).toBe(BASE + 2);

		// A hint of a position the device already holds asks for nothing.
		await h.message(libraryChanged(BASE + 2));
		await h.idle();
		expect((await h.api.asked('/')).length).toBe(asked + 2);
	});

	it('a catch-up interrupted mid-way resumes from its last page', async () => {
		await started('1');
		await grown();
		await h.api.setPageSize(1);
		const feed = [
			tag(BASE + 1, '90', 'Rock'),
			tag(BASE + 2, '91', 'Jazz'),
			tag(BASE + 3, '92', 'Soul')
		];
		await h.api.setFeed('1', feed, BASE + 3);
		// Two requests of the feed were made before this catch-up, and two pages of it are served.
		await h.api.failAfter('/libraries/1/changes', 4);

		await h.message(libraryChanged(BASE + 3));
		await h.idle();
		// The two pages are kept but not shown, and the failure is told.
		expect(await tagName('90')).toBeUndefined();
		expect([await h.cursor('1', 'library'), await h.pulled('1', 'library')]).toEqual([
			BASE,
			BASE + 2
		]);
		expect(await h.reported()).toHaveLength(1);

		await h.api.healed();
		const asked = (await h.api.asked('/')).length;
		await h.start();
		await h.idle();
		const resumed = (await h.api.asked('/'))
			.slice(asked)
			.filter((p) => p.includes('/libraries/1/'));
		expect(resumed).toEqual([`/libraries/1/changes?after=${BASE + 2}&epoch=1&limit=1000`]);
		expect([await tagName('90'), await tagName('91'), await tagName('92')]).toEqual([
			'Rock',
			'Jazz',
			'Soul'
		]);
		expect(await h.cursor('1', 'library')).toBe(BASE + 3);
	});

	it('a hint during a pull triggers another pull', async () => {
		await started('1');
		await grown();
		await h.api.setPageSize(1);
		await h.api.setFeed('1', [tag(BASE + 1, '90', 'Rock')], BASE + 1);
		await h.api.hold('/libraries/1/changes');
		await h.message(libraryChanged(BASE + 1));
		await expect.poll(() => h.api.holding()).toBe(true);

		const feed = [tag(BASE + 1, '90', 'Rock'), tag(BASE + 2, '91', 'Jazz')];
		await h.api.setFeed('1', feed, BASE + 2);
		await h.message(libraryChanged(BASE + 2));
		await h.api.release();
		await h.idle();
		expect(await tagName('91')).toBe('Jazz');
		expect(await h.cursor('1', 'library')).toBe(BASE + 2);
		expect((await h.api.asked('/libraries/1/changes')).slice(2)).toEqual([
			`/libraries/1/changes?after=${BASE}&epoch=1&limit=1000`,
			`/libraries/1/changes?after=${BASE + 1}&epoch=1&limit=1000`
		]);
	});

	it('a library is on the device only once its snapshot is swapped in', async () => {
		await serve('1');
		await h.api.hold('/catalog');
		await h.start();
		await expect.poll(() => h.api.holding()).toBe(true);
		expect(await h.onDevice('1')).toBe(false);
		await h.api.release();
		await h.idle();
		expect(await h.onDevice('1')).toBe(true);
		expect(await h.onDevice('2')).toBe(false);
	});

	it('queries made while a new library makes room in the pool wait for it and do not fail', async () => {
		await started('1');
		await serve('1', '2');
		await h.message(hello);
		// The new library's bootstrap reserves pool files and opens its file; the queries land throughout.
		const answers = Array.from({ length: 150 }, () => h.query('1', 'listTracks', { limit: 1 }));
		await expect(Promise.all(answers)).resolves.toHaveLength(150);
		await h.idle();
		expect(await h.libraries()).toEqual(['1', '2']);
		expect(await h.reported()).toEqual([]);
	});

	it('hints for a library arrive while others bootstrap, and none fails', async () => {
		await started('1');
		await grown();
		const others = Array.from({ length: 5 }, (_, i) => `${i + 2}`);
		await h.api.setLibraries(['1', ...others]);
		for (const id of others) {
			await h.api.setSeq(id, SEQ);
			await h.api.setFeed(id, [], SEQ);
		}
		await h.api.setPageSize(1);
		await h.message(hello);
		// Each hint is a further page of changes, staged and applied while the others bootstrap.
		for (let i = 0; i < 40; i++) {
			const seq = BASE + 1 + i;
			const changes = Array.from({ length: i + 1 }, (_, j) =>
				tag(BASE + 1 + j, `${90 + j}`, `Tag ${j}`)
			);
			await h.api.setFeed('1', changes, seq);
			await h.message(libraryChanged(seq));
		}
		await h.idle();
		expect(await h.reported()).toEqual([]);
		expect(await h.libraries()).toEqual(['1', ...others]);
		expect(await tagName('129')).toBe('Tag 39');
	});

	it('a corrupt or missing database bootstraps again', async () => {
		await started('1');
		await h.stop();
		const [file] = await h.files();
		await h.remove(file);
		await h.start();
		await h.idle();
		expect(await h.libraries()).toEqual(['1']);
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);

		// A file that no longer opens, as SQLite finds its schema malformed.
		await h.stop();
		const [again] = await h.files();
		await h.run(again, [
			'PRAGMA writable_schema = ON',
			"UPDATE sqlite_schema SET sql = 'CREATE TABLE feed_cursors (' WHERE name = 'feed_cursors'"
		]);
		await h.start();
		await h.idle();
		expect(await h.libraries()).toEqual(['1']);
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(3);
		expect(await total('listTracks')).toBe(6);
		expect(await h.files()).toHaveLength(1);
	});

	it('a database of an older schema version is deleted and bootstrapped again', async () => {
		await started('1');
		await h.stop();
		const [file] = await h.files();
		await h.run(file, ['PRAGMA user_version = 1']);

		await h.start();
		await h.idle();
		expect(await h.libraries()).toEqual(['1']);
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await total('listTracks')).toBe(6);
	});

	it('a database of a newer schema version is kept for the build that wrote it, and its library left to the server', async () => {
		await started('1');
		await h.stop();
		const [file] = await h.files();
		await h.run(file, ['PRAGMA user_version = 99']);

		await h.start();
		await h.idle();
		expect(await h.files()).toEqual([file]);
		expect(await h.libraries()).toEqual([]);
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(1);
	});

	it('a corruption error from a query bootstraps that library again', async () => {
		await started('1', '2');
		await h.damage('1');
		await expect(h.query('1', 'listTracks', {})).rejects.toThrow(/malformed/);
		await h.idle();

		expect(await h.libraries()).toEqual(['1', '2']);
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await h.api.asked('/libraries/2/catalog')).toHaveLength(1);
		expect(await total('listTracks')).toBe(6);
	});

	it('a library no longer listed is deleted', async () => {
		await started('1', '2');
		expect(await h.files()).toHaveLength(2);

		await h.api.setLibraries(['1']);
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await h.idle();
		expect(await h.libraries()).toEqual(['1']);
		expect(await h.files()).toHaveLength(1);
	});

	it('a newly listed library is bootstrapped', async () => {
		await started('1');
		await h.api.setLibraries(['1', '2']);
		await h.api.setSeq('2', 7);
		await h.api.setFeed('2', [], 7);
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await h.idle();

		expect(await h.libraries()).toEqual(['1', '2']);
		expect(await h.cursor('2', 'library')).toBe(7);
		expect(await h.files()).toHaveLength(2);
	});

	it('sign-out deletes the database', async () => {
		await started('1', '2');
		expect(await h.files()).toHaveLength(2);

		await h.signOut();
		expect(await h.files()).toEqual([]);
		expect(await h.libraries()).toEqual([]);
	});

	it('a file kept for another account is deleted', async () => {
		await started('1');
		await h.stop();
		await h.api.setSession('2');
		await h.start('2');
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await h.files()).toHaveLength(1);
	});

	it('a worker whose session is now another account’s pulls nothing of it', async () => {
		await started('1');
		await h.api.setSession('2');
		await h.api.setLibraries(['1', '2']);
		await h.message(hello);
		await h.idle();
		expect(await h.api.asked('/libraries/2')).toEqual([]);
		expect(await h.api.asked('/me/changes')).toHaveLength(1);
		expect(await h.libraries()).toEqual(['1']);
	});

	it('a worker whose session is now another account’s downloads no library again', async () => {
		await started('1');
		await h.api.setSession('2');
		await h.api.setSeq('1', 5);
		await h.api.setFeed('1', [], 5);
		await h.message(libraryChanged(SEQ + 1));
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(1);
		expect(await h.libraries()).toEqual(['1']);
	});

	it('410 bootstraps again', async () => {
		await started('1');
		// The server's feed is now behind the device's position, as after a restored backup.
		await h.api.setSeq('1', 5);
		await h.api.setFeed('1', [], 5);
		await h.message(libraryChanged(SEQ + 1));
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await h.cursor('1', 'library')).toBe(5);
		expect(await h.libraries()).toEqual(['1']);
	});

	it('404 from a feed deletes the library, which the account no longer reaches', async () => {
		await started('1', '2');
		await h.api.setLibraries(['1']);
		await h.message(libraryChanged(SEQ + 1, '2'));
		await h.idle();

		expect(await h.libraries()).toEqual(['1']);
		expect(await h.files()).toHaveLength(1);
	});

	it('410 from the account feed bootstraps the library again', async () => {
		await started('1');
		expect(await total('listPlaylists')).toBe(2);
		// The server's account feed is behind the device's position, and holds no playlists now.
		await h.api.setAccount([], 1);
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await total('listPlaylists')).toBe(0);
		expect(await h.cursor('1', 'account')).toBe(1);
	});

	it('410 keeps the old copy until the new snapshot is in', async () => {
		await started('1');
		await h.api.setSeq('1', 5);
		await h.api.setFeed('1', [], 5);
		await h.api.failAfter('/libraries/1/catalog', 1);
		await h.message(libraryChanged(SEQ + 1));
		await h.idle();

		expect(await h.libraries()).toEqual(['1']);
		expect(await total('listTracks')).toBe(6);
		expect(await h.reported()).toHaveLength(1);
	});

	it("asks each feed from the epoch of its last page, and takes on a restarted server's", async () => {
		await started('1');
		await grown();
		expect([await h.epoch('1', 'library'), await h.epoch('1', 'account')]).toEqual(['1', '1']);

		await h.api.restart('2');
		await h.api.setFeed('1', [tag(BASE + 1, '90', 'Rock')], BASE + 1);
		await h.message(libraryChanged(BASE + 1));
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await h.idle();
		expect(await h.api.asked(`/libraries/1/changes?after=${BASE}&epoch=1`)).toHaveLength(1);
		expect(await h.api.asked(`/me/changes?after=${accountHead}&epoch=1`)).toHaveLength(1);
		expect([await h.epoch('1', 'library'), await h.epoch('1', 'account')]).toEqual(['2', '2']);

		await h.api.setFeed('1', [tag(BASE + 2, '91', 'Jazz')], BASE + 2);
		await h.message(libraryChanged(BASE + 2));
		await h.idle();
		expect(await h.api.asked(`/libraries/1/changes?after=${BASE + 1}&epoch=2`)).toHaveLength(1);
		expect(await tagName('91')).toBe('Jazz');
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(1);
	});

	it('a restored server that does not know the epoch is downloaded again', async () => {
		await started('1');
		// The restored feed has moved past the device's position, so only the epoch tells.
		await h.api.restart('9', true);
		await h.api.setSeq('1', SEQ + 5);
		await h.api.setFeed('1', [], SEQ + 5);
		await h.message(libraryChanged(SEQ + 5));
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect([await h.cursor('1', 'library'), await h.epoch('1', 'library')]).toEqual([SEQ + 5, '9']);
		expect(await h.reported()).toEqual([]);
	});

	it('a file kept before feeds had epochs is deleted and bootstrapped again', async () => {
		await started('1');
		await h.stop();
		const [file] = await h.files();
		// Schema version 4 kept a feed's position alone.
		await h.run(file, [
			'DROP TABLE feed_cursors',
			'CREATE TABLE feed_cursors (feed TEXT PRIMARY KEY, seq INTEGER NOT NULL) WITHOUT ROWID',
			`INSERT INTO feed_cursors VALUES ('library', ${SEQ}), ('account', ${accountHead}), ('owner', 1)`,
			'PRAGMA user_version = 4'
		]);

		await h.start();
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		await h.api.setFeed('1', [tag(SEQ + 1, '90', 'Rock')], SEQ + 1);
		await h.message(libraryChanged(SEQ + 1));
		await h.idle();
		expect(await tagName('90')).toBe('Rock');
		expect(await h.reported()).toEqual([]);
	});

	it('a snapshot whose hash does not match its ETag is discarded and downloaded again', async () => {
		await serve('1');
		await h.api.wrongEtag(1);
		await h.start();
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await h.libraries()).toEqual(['1']);
		expect(await h.files()).toHaveLength(1);
		expect(await h.reported()).toEqual([]);
	});

	it('a snapshot that never matches its ETag is never shown', async () => {
		await serve('1');
		await h.api.wrongEtag(5);
		await h.start();
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(3);
		expect(await h.libraries()).toEqual([]);
		expect(await h.files()).toEqual([]);
		expect(await h.reported()).toHaveLength(1);
	});

	it('a catch-up over 10% of the library downloads the snapshot', async () => {
		await started('1');
		// The library holds 14 tracks, albums and artists: one change is a catch-up, three are not.
		await h.api.setFeed('1', [tag(11, '90', 'Rock')], 11);
		await h.message(libraryChanged(11));
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(1);
		expect(await tagName('90')).toBe('Rock');

		const more = [tag(12, '91', 'Jazz'), tag(13, '92', 'Soul'), tag(14, '93', 'Folk')];
		await h.api.setFeed('1', more, 14);
		// As the server does, the first ask gets the stale snapshot and starts a newer one.
		await h.api.rebuild('1', 14);
		await h.message(libraryChanged(14));
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(3);
		expect(await h.cursor('1', 'library')).toBe(14);
		// The changes came with the newer snapshot, which this one stands in for, not the feed.
		expect(await tagName('91')).toBeUndefined();
		expect(await h.libraries()).toEqual(['1']);
		// The copy it replaced is gone, so the storage is not doubled.
		expect(await h.files()).toHaveLength(1);
	});

	it("a re-downloaded snapshot keeps the library's playlists", async () => {
		await started('1');
		expect(await total('listPlaylists')).toBe(2);

		const more = [tag(12, '91', 'Jazz'), tag(13, '92', 'Soul'), tag(14, '93', 'Folk')];
		await h.api.setFeed('1', more, 14);
		await h.api.setSeq('1', 14);
		await h.message(libraryChanged(14));
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await h.api.asked('/me/changes?after=0')).toHaveLength(2);
		expect(await total('listPlaylists')).toBe(2);
		expect(await h.cursor('1', 'account')).toBe(accountHead);
	});

	it("takes only a library's own playlists from the account feed, and later changes to them", async () => {
		await started('1');
		expect(await total('listPlaylists')).toBe(2);
		// The playlist of library 2 is in no row of library 1's file.
		expect(await h.queryAs('1', '2', 'listPlaylists', {})).toMatchObject({ total: 0 });

		const renamed = structuredClone(playlists[0]);
		(renamed.rows.playlists[0] as { title: string }).title = 'Renamed';
		renamed.seq = accountHead + 1;
		const deleted = { seq: accountHead + 2, op: 'delete', kind: 'playlist', id: '502' };
		await h.api.setAccount([renamed, deleted], accountHead + 2);
		await h.message({ type: 'accountChanged', seq: accountHead + 2 });
		await h.idle();
		const titles = (await h.query('1', 'listPlaylists', {})) as { items: { title: string }[] };
		expect(titles.items.map((item) => item.title)).toEqual(['Renamed']);
		expect(await h.cursor('1', 'account')).toBe(accountHead + 2);
	});

	it('announces a library when its rows change: a snapshot, a catch-up, its playlists, and not a quiet pull', async () => {
		await started('1');
		expect(await h.changes()).toEqual(['1']);

		await h.message(libraryChanged(SEQ));
		await h.idle();
		expect(await h.changes()).toEqual(['1']);

		await h.api.setFeed('1', [tag(11, '90', 'Rock')], 11);
		await h.message(libraryChanged(11));
		await h.idle();
		expect(await h.changes()).toEqual(['1', '1']);

		// Another library's playlist is not this library's change.
		const other = { ...elsewhere, seq: accountHead + 1 };
		await h.api.setAccount([other], accountHead + 1);
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await h.idle();
		expect(await h.changes()).toEqual(['1', '1']);

		const renamed = structuredClone(playlists[0]);
		renamed.seq = accountHead + 2;
		await h.api.setAccount([renamed], accountHead + 2);
		await h.message({ type: 'accountChanged', seq: accountHead + 2 });
		await h.idle();
		expect(await h.changes()).toEqual(['1', '1', '1']);
	});

	it('a 503 catalog_building waits and asks again', async () => {
		await serve('1');
		await h.api.building(2);
		await h.start();
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(3);
		expect(await h.libraries()).toEqual(['1']);
	});

	it('another library answers while a snapshot downloads', async () => {
		await started('1');
		await h.api.setLibraries(['1', '2']);
		await h.api.setSeq('2', 7);
		await h.api.setFeed('2', [], 7);
		await h.api.holdBody();
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await expect.poll(() => h.api.bodyHeld()).toBe(true);

		expect(await h.busy()).toBe(true);
		expect(await total('listTracks')).toBe(6);
		expect(await h.files()).toHaveLength(1);
		await h.api.setFeed('1', [tag(11, '90', 'Rock')], 11);
		await h.message(libraryChanged(11));
		await expect.poll(() => tagName('90')).toBe('Rock');
		expect(await h.libraries()).toEqual(['1']);

		await h.api.releaseBody();
		await h.idle();
		expect(await h.busy()).toBe(false);
		expect(await h.libraries()).toEqual(['1', '2']);
		expect(await h.files()).toHaveLength(2);
	});

	it('one change after a gap in the sequence stays a catch-up', async () => {
		await started('1');
		await grown();
		// Other libraries used the numbers in between.
		await h.api.setFeed('1', [tag(BASE + 500, '90', 'Rock')], BASE + 500);
		await h.message(libraryChanged(BASE + 500));
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(1);
		expect(await tagName('90')).toBe('Rock');
	});

	it('a far catch-up whose snapshot stays no newer is made from the feed, after asking again', async () => {
		await started('1');
		await grown();
		const [file] = await h.files();
		await h.api.setPageSize(150);
		const many = Array.from({ length: 300 }, (_, i) => tag(BASE + 1 + i, `${100 + i}`, `Tag ${i}`));
		await h.api.setFeed('1', many, BASE + 300);
		await h.message(libraryChanged(BASE + 300));
		await h.idle();

		// The snapshot stays at SEQ, behind the copy's position: asked for while the server might
		// build a newer one, then never imported.
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(11);
		expect(await h.files()).toEqual([file]);
		expect(await h.cursor('1', 'library')).toBe(BASE + 300);
		expect(await tagName('399')).toBe('Tag 299');
	});

	it('a hint during a pull that brings nothing is not lost', async () => {
		await started('1');
		await h.api.hold('/libraries/1/changes');
		await h.message(hello);
		await expect.poll(() => h.api.holding()).toBe(true);

		await h.api.setFeed('1', [tag(11, '90', 'Rock')], 11);
		await h.message(libraryChanged(11));
		await h.api.release();
		await h.idle();
		expect(await tagName('90')).toBe('Rock');

		const renamed = structuredClone(playlists[0]);
		(renamed.rows.playlists[0] as { title: string }).title = 'Renamed';
		renamed.seq = accountHead + 1;
		await h.api.hold('/me/changes');
		await h.message(hello);
		await expect.poll(() => h.api.holding()).toBe(true);
		await h.api.setAccount([renamed], accountHead + 1);
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await h.api.release();
		await h.idle();
		expect(await h.cursor('1', 'account')).toBe(accountHead + 1);
	});

	it('a bootstrap that keeps failing is tried again only at a start or a hello', async () => {
		await serve('1');
		await h.api.wrongEtag(10);
		await h.start();
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(3);

		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await h.message(libraryChanged(SEQ + 1));
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(3);

		await h.message(hello);
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(6);
	});

	it('changes pulled add up across pages towards the 10% that downloads the snapshot', async () => {
		await started('1');
		await grown();
		await h.api.setPageSize(60);
		const many = Array.from({ length: 120 }, (_, i) => tag(BASE + 1 + i, `${100 + i}`, `Tag ${i}`));
		await h.api.setFeed('1', many, BASE + 120);
		await h.api.setSeq('1', BASE + 120);
		await h.message(libraryChanged(BASE + 120));
		await h.idle();

		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await h.cursor('1', 'library')).toBe(BASE + 120);
	});

	it('clears the downloads a closed worker left behind', async () => {
		await started('1');
		await h.stop();
		await h.leaveDownload('download-1.tmp');
		expect(await h.downloads()).toEqual(['download-1.tmp']);
		await h.start();
		await h.idle();
		expect(await h.downloads()).toEqual([]);
	});

	it("a new library's download does not hold up the playlists of the libraries held", async () => {
		await started('1');
		await h.api.setLibraries(['1', '2']);
		await h.api.setSeq('2', 7);
		await h.api.setFeed('2', [], 7);
		await h.api.holdBody();
		const renamed = structuredClone(playlists[0]);
		(renamed.rows.playlists[0] as { title: string }).title = 'Renamed';
		renamed.seq = accountHead + 1;
		await h.api.setAccount([renamed], accountHead + 1);
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await expect.poll(() => h.api.bodyHeld()).toBe(true);

		await expect.poll(() => h.cursor('1', 'account')).toBe(accountHead + 1);
		await h.api.releaseBody();
		await h.idle();
		expect(await h.libraries()).toEqual(['1', '2']);
	});

	it('a large catch-up applies in several chunks and stays invisible until the end', async () => {
		const before = await largeCatchUp();
		const mid = await h.midApply('1', 'listTags', { limit: 1 });
		expect(mid.partial.total).toBeGreaterThan(before);
		expect(mid.partial.total).toBeLessThan(before + LARGE);
		expect(mid.through).toBe(BASE + LARGE);
		// A person's read made meanwhile answers only once all of it is in.
		expect(mid.offline.total).toBe(before + LARGE);

		await h.idle();
		expect(await h.onDevice('1')).toBe(true);
		expect(await h.cursor('1', 'library')).toBe(BASE + LARGE);
	});

	it('a query during an apply throws at once online, and offline waits for the apply', async () => {
		const before = await largeCatchUp();
		const mid = await h.midApply('1', 'listTags', { limit: 1 });
		expect(mid.online).toEqual({ error: 'the library is being updated on this device' });
		expect(mid.offline.total).toBe(before + LARGE);
		await h.idle();
		expect(await total('listTags')).toBe(before + LARGE);
	});

	it('an apply a closed worker left part-way is finished before the library answers', async () => {
		const before = await largeCatchUp();
		await h.stopMidApply('1');
		const asked = (await h.api.asked('/')).length;

		await h.start();
		await expect(h.query('1', 'listTags', { limit: 1 })).rejects.toThrow(/being updated/);
		expect(await h.query('1', 'listTags', { limit: 1 }, true)).toMatchObject({
			total: before + LARGE
		});
		await h.idle();
		expect(await h.cursor('1', 'library')).toBe(BASE + LARGE);
		expect(await h.pulled('1', 'library')).toBe(BASE + LARGE);
		// Nothing of the catch-up was pulled again: the finish and the start's pull ask after it.
		const pulls = (await h.api.asked('/'))
			.slice(asked)
			.filter((p) => p.includes('/libraries/1/changes'));
		expect(new Set(pulls)).toEqual(
			new Set([`/libraries/1/changes?after=${BASE + LARGE}&epoch=1&limit=1000`])
		);
	});

	it('a catch-up that will not apply bootstraps again, and its part is never read', async () => {
		await started('1');
		await grown(100 * LARGE);
		const before = await total('listTags');
		const feed = largeFeed();
		// A NULL name breaks the table's constraint, in the last chunk.
		(feed[LARGE - 1].rows.tags[0] as { name: string | null }).name = null;
		await h.api.setFeed('1', feed, BASE + LARGE);
		// The snapshot downloaded again is current to the feed's head.
		await h.api.setSeq('1', BASE + LARGE);
		await h.message(libraryChanged(BASE + LARGE));

		const mid = await h.midApply('1', 'listTags', { limit: 1 });
		expect(mid.partial.total).toBeGreaterThan(before);
		expect(mid.online).toEqual({ error: 'the library is being updated on this device' });
		expect(mid.offline).toEqual({ error: 'the library is not on this device' });
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(2);
		expect(await h.cursor('1', 'library')).toBe(BASE + LARGE);
		expect(await total('listTags')).toBe(before);
		expect(await h.files()).toHaveLength(1);
	});

	it('a full device during an apply keeps the copy, and a later pull finishes it', async () => {
		await started('1');
		await grown();
		await h.api.setFeed('1', [tag(BASE + 1, '90', 'Rock')], BASE + 1);
		await h.failNextCommit('database or disk is full');
		await h.message(libraryChanged(BASE + 1));
		await h.idle();
		expect(await h.reported()).toMatchObject([{ quota: true }]);
		expect(await h.libraries()).toEqual(['1']);
		expect(await h.files()).toHaveLength(1);
		expect(await h.onDevice('1')).toBe(false);
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(1);

		await h.message(libraryChanged(BASE + 1));
		await h.idle();
		expect(await h.onDevice('1')).toBe(true);
		expect(await tagName('90')).toBe('Rock');
		expect(await h.cursor('1', 'library')).toBe(BASE + 1);
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(1);
	});

	it('a copy whose server moved to another schema version answers only offline', async () => {
		await started('1');
		// Three changes are more than a tenth of the library, so its snapshot is asked for.
		const more = [tag(11, '91', 'Jazz'), tag(12, '92', 'Soul'), tag(13, '93', 'Folk')];
		await h.api.setFeed('1', more, 13);
		await h.api.setSchema(99);
		await h.message(libraryChanged(13));
		await h.idle();
		expect(await h.reported()).toHaveLength(1);
		expect(await h.libraries()).toEqual(['1']);
		await expect(total('listTracks')).rejects.toThrow(/being updated/);
		expect(await h.query('1', 'listTracks', { limit: 1 }, true)).toMatchObject({ total: 6 });
	});

	it('a snapshot of another schema version is not imported, and is not asked for again', async () => {
		await serve('1');
		await h.api.setSchema(99);
		await h.start();
		await h.idle();
		expect(await h.libraries()).toEqual([]);
		expect(await h.files()).toEqual([]);
		expect(await h.reported()).toHaveLength(1);

		await h.message(hello);
		await h.message(libraryChanged(SEQ + 1));
		await h.message({ type: 'accountChanged', seq: accountHead + 1 });
		await h.idle();
		expect(await h.api.asked('/libraries/1/catalog')).toHaveLength(1);
		expect(await h.reported()).toHaveLength(1);
	});

	it('pulls on hello', async () => {
		await started('1');
		await h.api.setFeed('1', [tag(11, '90', 'Rock')], 11);
		await h.message(hello);
		await h.idle();
		expect(await tagName('90')).toBe('Rock');
	});

	it('device search equals the golden server results', async () => {
		await started('1');
		// Recipes first, so a search recipe is what builds the index.
		const searches = [
			...recipeCases.filter((c) => c.params.recipe?.type === 'search'),
			...searchCases
		];
		expect(searches.length).toBeGreaterThan(10);
		for (const { query, params, answer } of searches)
			expect(await h.query('1', query, params), JSON.stringify(params)).toEqual(answer);
	});

	it('a search finds what a catch-up added once the index is rebuilt', async () => {
		await started('1');
		const found = async () =>
			(
				(await h.query('1', 'search', { q: 'zebra', sectionLimit: 5 })) as {
					sections: unknown[];
				}
			).sections.length;
		expect(await found()).toBe(0);
		await h.api.setFeed('1', [album(SEQ + 1, '900', 'Zebra Crossing')], SEQ + 1);
		await h.message(libraryChanged(SEQ + 1));
		await h.idle();
		await expect.poll(async () => await found(), { timeout: 5000 }).toBe(1);
	});
});
