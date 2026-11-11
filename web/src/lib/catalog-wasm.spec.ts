import { readFileSync } from 'node:fs';
import { expect, test } from 'vitest';
import { Catalog } from '$lib/catalog-wasm/jewelcase_catalog_wasm.js';

test('catalog-wasm opens an in-memory database and creates the schema', () => {
	const catalog = Catalog.openInMemory();
	expect(catalog.schemaVersion()).toBe(4);
	expect(catalog.tables()).toEqual(
		expect.arrayContaining([
			'albums',
			'artists',
			'images',
			'libraries',
			'tags',
			'tracks',
			'track_tags',
			'playlists',
			'playlist_items'
		])
	);
	catalog.free();
});

test('the staging table is a rowid table', () => {
	const catalog = Catalog.openInMemory();
	expect(() => catalog.sql('SELECT rowid FROM staged_changes')).not.toThrow();
	expect(() => catalog.sql('SELECT rowid FROM feed_cursors')).toThrow();
	catalog.free();
});

/** The parsed golden file `name` from the catalog crate's parity folder. */
const parity = (name: string) =>
	JSON.parse(
		readFileSync(new URL(`../../../crates/catalog/parity/${name}`, import.meta.url), 'utf8')
	);

/** An entity's rows in `library.json`, as the parity test exported them from the server. */
type Rows = { kind: string; id: string; key: number; tables: Record<string, unknown[]> };

/**
 * A device database built from `library.json` as a device syncs it: each entity's rows become an
 * upsert in its feed's response, staged and committed, the playlists from the account feed and
 * the rest from the library's. The search index is built.
 */
function device(): Catalog {
	const catalog = Catalog.openInMemory();
	const feeds: Record<string, Rows[]> = { 'library:1': [], account: [] };
	for (const rows of parity('library.json') as Rows[]) {
		feeds[rows.kind.startsWith('playlist') ? 'account' : 'library:1'].push(rows);
	}
	for (const [feed, entities] of Object.entries(feeds)) {
		const changes = entities.map(({ kind, id, key, tables }, at) => ({
			seq: at + 1,
			op: 'upsert',
			kind,
			id,
			key,
			rows: tables
		}));
		catalog.stage(feed, JSON.stringify(changes));
		catalog.commit(feed, BigInt(changes.length), changes.length);
	}
	catalog.buildIndex('1');
	return catalog;
}

/** One golden case: a query, its parameters and the JSON the server answered. */
type Case = { query: string; params: Record<string, unknown>; answer: unknown };

/**
 * The parsed answer to a golden case's query, or `{ error }` when it throws, as the parity test
 * records a refused playlist cursor.
 */
function ask(catalog: Catalog, { query, params }: Case): unknown {
	try {
		return JSON.parse(catalog.query('1', '1', query, JSON.stringify(params)));
	} catch (error) {
		return { error: (error as Error).message };
	}
}

test('catalog-wasm reproduces the golden parity file', () => {
	const catalog = device();
	let cases = 0;
	for (const file of ['tracks', 'browse', 'playlists', 'search', 'recipes']) {
		for (const golden of parity(`${file}.json`) as Case[]) {
			expect(ask(catalog, golden), `${golden.query} ${JSON.stringify(golden.params)}`).toEqual(
				golden.answer
			);
			cases++;
		}
	}
	expect(cases).toBeGreaterThan(200);
	catalog.free();
});

test('catalog-wasm refuses what it cannot answer with an error', () => {
	const catalog = Catalog.openInMemory();
	const query = (name: string, params: object) => () =>
		catalog.query('1', '1', name, JSON.stringify(params));
	expect(query('noSuchQuery', {})).toThrow('unknown query noSuchQuery');
	expect(query('listTracks', { sort: 'nope' })).toThrow('invalid params');
	expect(query('listAlbums', { limit: 0 })).toThrow('limit');
	expect(query('getTrack', { trackId: 'x' })).toThrow('trackId is not an ID');
	expect(() => catalog.query('1', '1', 'getTrack', '{')).toThrow('invalid params');
	expect(query('search', { q: 'a', sectionLimit: 5 })).toThrow('search index is not built');
	expect(query('resolveRecipe', { recipe: { type: 'search', q: 'a', filters: {} } })).toThrow(
		'search index is not built'
	);
	expect(() => catalog.stage('library:1', '[{"kind":"track"}]')).toThrow();
	catalog.free();
});

/** A tag entity with its rows, as a feed's upsert carries it. */
const tag = (name: string, id = '9') => ({
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
/** A feed change at `seq` that upserts the tag `id` named `name`. */
const upsert = (seq: number, name: string, id = '9') => ({
	seq,
	op: 'upsert',
	...tag(name, id)
});

test('catalog-wasm applies staged changes only at commit, with the feed position', () => {
	const catalog = Catalog.openInMemory();
	const name = () => JSON.parse(catalog.query('1', '1', 'getTag', '{"tagId":"9"}'))?.name;
	expect(catalog.cursor('library:1')).toBe(0n);

	catalog.stage('library:1', JSON.stringify([upsert(3, 'Rock')]));
	catalog.stage('library:1', JSON.stringify([upsert(4, 'Rock and roll')]));
	expect([name(), catalog.cursor('library:1'), catalog.pulled('library:1')]).toEqual([
		undefined,
		0n,
		4n
	]);

	catalog.commit('library:1', 4n, 100);
	expect([name(), catalog.cursor('library:1'), catalog.pulled('library:1')]).toEqual([
		'Rock and roll',
		4n,
		4n
	]);

	catalog.stage(
		'library:1',
		JSON.stringify([{ seq: 5, op: 'delete', kind: 'tag', id: '9', rows: null }])
	);
	expect(name()).toBe('Rock and roll');
	catalog.commit('library:1', 5n, 100);
	expect([name(), catalog.cursor('library:1'), catalog.cursor('account')]).toEqual([
		undefined,
		5n,
		0n
	]);
	catalog.free();
});

test('catalog-wasm applies staged changes a chunk at a time, marked until the last', () => {
	const catalog = Catalog.openInMemory();
	const tags = () => JSON.parse(catalog.query('1', '1', 'listTags', '{"limit":1}')).total;
	catalog.stage(
		'library:1',
		JSON.stringify([upsert(3, 'Rock', '10'), upsert(4, 'Jazz', '11'), upsert(5, 'Soul', '12')])
	);

	expect(catalog.commit('library:1', 6n, 2)).toBe(false);
	expect([tags(), catalog.applying('library:1'), catalog.cursor('library:1')]).toEqual([2, 6n, 0n]);
	// Only what is left is applied: the first chunk's changes were deleted with it.
	expect(catalog.pulled('library:1')).toBe(5n);

	expect(catalog.commit('library:1', 6n, 2)).toBe(true);
	expect([tags(), catalog.applying('library:1'), catalog.cursor('library:1')]).toEqual([
		3,
		undefined,
		6n
	]);
	catalog.free();
});

test('catalog-wasm takes the API defaults for the parameters a request leaves out', () => {
	const catalog = device();
	const ask = (name: string, params: object) =>
		JSON.parse(catalog.query('1', '1', name, JSON.stringify(params)));
	const golden = (name: string, params: object) =>
		(parity('browse.json') as Case[]).find(
			(c) => c.query === name && JSON.stringify(c.params) === JSON.stringify(params)
		)!.answer;
	expect(ask('listAlbums', {})).toEqual(
		golden('listAlbums', { limit: 100, order: 'asc', sort: 'name' })
	);
	expect(ask('listAlbums', { sort: 'name' })).toEqual(ask('listAlbums', {}));
	expect(ask('listArtists', {})).toEqual(ask('listArtists', { sort: 'name', order: 'asc' }));
	expect(ask('listTags', {})).toEqual(ask('listTags', { sort: 'name', order: 'asc' }));
	// Road Trip, Quiet and Liked Songs; Abbey Road without "Bonus", which the account hid.
	expect(ask('listPlaylists', {}).total).toBe(3);
	expect(ask('listTracks', { albumId: '101', sort: 'album', limit: 1000 }).items.length).toBe(3);
	expect(ask('search', { q: 'a' })).toEqual(ask('search', { q: 'a', sectionLimit: 5 }));
	catalog.free();
});

test('catalog-wasm search keeps only what carries the tags', () => {
	const catalog = device();
	const ids = (params: object) =>
		JSON.parse(
			catalog.query('1', '1', 'search', JSON.stringify({ q: 'a', ...params }))
		).sections.map((section: { items: { track?: { id: string } }[] }) => section.items.length);
	expect(ids({}).length).toBeGreaterThan(0);
	expect(ids({ tagAll: '999' })).toEqual([]);
	const tracks = JSON.parse(
		catalog.query('1', '1', 'search', JSON.stringify({ q: 'come', types: 'tracks', tagAll: '30' }))
	).sections[0].items.map((item: { track: { id: string } }) => item.track.id);
	const tagged = JSON.parse(
		catalog.query('1', '1', 'listTracks', JSON.stringify({ tagAll: '30' }))
	).items.map((track: { id: string }) => track.id);
	expect(tracks.length).toBeGreaterThan(0);
	expect(tagged).toEqual(expect.arrayContaining(tracks));
	catalog.free();
});

test('catalog-wasm shows a counted total anew after a commit', () => {
	const catalog = device();
	/** The first page and total of the tracks carrying either of two tags, whose total is counted. */
	const page = () =>
		JSON.parse(
			catalog.query('1', '1', 'listTracks', JSON.stringify({ tagAny: '30,31', limit: 1 }))
		);
	const first = page();
	expect(first.total).toBe(1);
	expect(page().total).toBe(1);

	// The library feed's changes in `device()` are positions 1 to its length.
	const position = (parity('library.json') as Rows[]).filter(
		(rows) => !rows.kind.startsWith('playlist')
	).length;
	catalog.stage(
		'library:1',
		JSON.stringify([
			{ seq: position + 1, op: 'delete', kind: 'track', id: first.items[0].id, rows: null }
		])
	);
	expect(page().total).toBe(1);
	catalog.commit('library:1', BigInt(position + 1), 100);
	expect(page().total).toBe(0);
	catalog.free();
});

test('catalog-wasm commits nothing when a staged change fails, and only the feed asked for', () => {
	const catalog = Catalog.openInMemory();
	const name = (id: string) =>
		JSON.parse(catalog.query('1', '1', 'getTag', `{"tagId":"${id}"}`))?.name;
	const broken = upsert(4, 'Jazz', '10');
	// A NULL name breaks the table's constraint, after the first change was applied.
	broken.rows.tags[0].name = null as unknown as string;
	catalog.stage('library:1', JSON.stringify([upsert(3, 'Rock'), broken]));
	expect(() => catalog.commit('library:1', 4n, 100)).toThrow();
	expect(name('9')).toBeUndefined();
	expect([catalog.cursor('library:1'), catalog.pulled('library:1')]).toEqual([0n, 4n]);

	catalog.stage('library:1', JSON.stringify([upsert(4, 'Jazz', '10')]));
	catalog.stage('account', JSON.stringify([upsert(1, 'Blues', '11')]));
	catalog.commit('library:1', 7n, 100);
	expect([name('9'), name('10'), name('11')]).toEqual(['Rock', 'Jazz', undefined]);
	expect([catalog.cursor('library:1'), catalog.pulled('library:1')]).toEqual([7n, 7n]);
	expect([catalog.cursor('account'), catalog.pulled('account')]).toEqual([0n, 1n]);
	catalog.free();
});
