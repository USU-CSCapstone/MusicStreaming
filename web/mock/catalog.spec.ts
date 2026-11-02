import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Catalog } from './catalog';
import { Db } from './db';

// The scanner's schema, from the server's own migrations, holding library 1's three tracks on one
// album by one artist: track 11 is Rock and Calm, 12 Rock and Jazz, 13 Jazz and Calm. Library 2
// has track 21, which is Rock in its own tag 50, so nothing of library 1 may reach it.
const MIGRATIONS = join(import.meta.dirname, '../../crates/server/migrations');
const FIXTURE = `
	INSERT INTO libraries (id, name, created_at) VALUES (1, 'Music', 0), (2, 'Other', 0);
	INSERT INTO library_roots (id, library_id, path, created_at) VALUES (1, 1, '/m', 0), (2, 2, '/o', 0);
	INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at)
	VALUES (301, 1, 'Aurora', 'aurora', x'61', 0), (302, 2, 'Elsewhere', 'elsewhere', x'65', 0);
	INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key, artist_sort_key, added_at)
	VALUES (101, 1, 'Signal', 'signal', 'aurora', x'73', x'61', 0),
	       (201, 2, 'Far', 'far', 'elsewhere', x'66', x'65', 0);
	INSERT INTO album_artists (library_id, album_id, position, artist_id) VALUES (1, 101, 0, 301), (2, 201, 0, 302);
	WITH t (id, library_id, album_id, root_id) AS (VALUES (11, 1, 101, 1), (12, 1, 101, 1), (13, 1, 101, 1), (21, 2, 201, 2))
	INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime, title, sort_key,
	                    artist_sort_key, album_sort_key, file_artist_sort_key, file_album_artist_sort_key,
	                    file_album_sort_key, codec, container, lossless, sample_rate_hz, channels,
	                    duration_us, added_at)
	SELECT id, library_id, album_id, root_id, id || '.flac', 1, 0, 'Track ' || id, CAST(id AS BLOB),
	       x'', x'', x'', x'', x'', 'flac', 'flac', 1, 44100, 2, 1000000, id FROM t;
	INSERT INTO track_artists (library_id, track_id, position, artist_id, artist_name)
	VALUES (1, 11, 0, 301, 'Aurora'), (1, 12, 0, 301, 'Aurora'), (1, 13, 0, 301, 'Aurora'),
	       (2, 21, 0, 302, 'Elsewhere');
	INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
	    (30, 1, 'genre', 'Rock', 'rock', x'726f636b'), (31, 1, 'genre', 'Jazz', 'jazz', x'6a617a7a'),
	    (32, 1, 'mood', 'Calm', 'calm', x'63616c6d'), (50, 2, 'genre', 'Rock', 'rock', x'726f636b');
	INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES
	    (1, 11, 30, 'Rock'), (1, 11, 32, 'Calm'), (1, 12, 30, 'Rock'), (1, 12, 31, 'Jazz'),
	    (1, 13, 31, 'Jazz'), (1, 13, 32, 'Calm'), (2, 21, 50, 'Rock');
`;

let dir: string;
let catalog: Catalog;

beforeAll(() => {
	dir = mkdtempSync(join(tmpdir(), 'jewelcase-mock-'));
	const path = join(dir, 'jewelcase.db');
	const db = new DatabaseSync(path);
	db.exec('PRAGMA foreign_keys = ON');
	for (const file of readdirSync(MIGRATIONS)
		.filter((f) => f.endsWith('.sql'))
		.sort()) {
		db.exec(readFileSync(join(MIGRATIONS, file), 'utf8'));
	}
	db.exec(FIXTURE);
	db.close();
	catalog = new Catalog(new Db(path));
});

afterAll(() => rmSync(dir, { recursive: true, force: true }));

/** The IDs of library 1's tracks that `query` lists. */
const tracks = (query: Record<string, string>) =>
	catalog.listTracks(1n, new URLSearchParams(query)).items.map((t) => t.id);

describe('the tag filter', () => {
	it('matches any of one type, and every type named', () => {
		expect(tracks({ tag: '30,31' })).toEqual(['11', '12', '13']);
		expect(tracks({ tag: '30,31,32' })).toEqual(['11', '13']);
		expect(tracks({ tag: '32' })).toEqual(['11', '13']);
	});

	it('matches all of one type with tagMatch=all', () => {
		expect(tracks({ tag: '30,31', tagMatch: 'all' })).toEqual(['12']);
	});

	it('matches nothing for a tag the library does not have', () => {
		expect(tracks({ tag: '30,50' })).toEqual([]);
		expect(tracks({ tag: 'nope' })).toEqual([]);
	});

	it('narrows a search, counting only what carries the tags', () => {
		const calm = new URLSearchParams({ tag: '32' });
		const found = catalog.searchTracks(1n, 'track', 10, calm);
		expect([found.items.map((t) => t.id), found.total]).toEqual([['11', '13'], 2]);
		expect(catalog.searchAlbums(1n, 'signal', 10, new URLSearchParams({ tag: '50' })).total).toBe(
			0
		);
	});

	it('narrows albums and artists by their tracks', () => {
		const q = new URLSearchParams({ tag: '31,30', tagMatch: 'all' });
		expect(catalog.listAlbums(1n, q).items.map((a) => a.id)).toEqual(['101']);
		expect(catalog.listArtists(1n, q).items.map((a) => a.id)).toEqual(['301']);
		const other = new URLSearchParams({ tag: '50' });
		expect(catalog.listAlbums(1n, other).items).toEqual([]);
	});
});

describe('tags', () => {
	it('come by type on summaries', () => {
		const [first] = catalog.listTracks(1n, new URLSearchParams({ tag: '32' })).items;
		expect(first.tags).toEqual({
			genre: [{ id: '30', name: 'Rock' }],
			mood: [{ id: '32', name: 'Calm' }]
		});
		expect(catalog.artist(1n, 301n)?.tags.genre?.map((t) => t.name)).toEqual(['Jazz', 'Rock']);
	});

	it('list their types in use, and browse by type', () => {
		expect(catalog.tagTypes(1n)).toEqual([
			{ key: 'genre', tagCount: 2 },
			{ key: 'mood', tagCount: 1 }
		]);
		const genres = catalog.listTags(1n, new URLSearchParams({ type: 'genre' }));
		expect(genres.items.map((t) => [t.name, t.trackCount, t.albumCount])).toEqual([
			['Jazz', 2, 1],
			['Rock', 2, 1]
		]);
		expect(catalog.tag(1n, 50n)).toBeNull();
	});
});
