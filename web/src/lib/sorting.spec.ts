import { describe, expect, it } from 'vitest';
import type { TrackSummary } from './api/types';
import { compareKeys, nextSort, serverSort, sortRows, sortTracks, type TrackSort } from './sorting';

type Fields = {
	artist?: string;
	album?: string;
	albumId?: string;
	disc?: number | null;
	n?: number | null;
	date?: string | null;
	duration?: number;
};

const track = (id: string, title: string, f: Fields = {}): TrackSummary => ({
	id,
	title,
	artists: [],
	album: { id: f.albumId ?? '100', title: null, artists: [], image: null },
	discNumber: f.disc ?? null,
	trackNumber: f.n ?? null,
	durationUs: f.duration ?? 1_000_000,
	releaseDate: f.date ?? null,
	explicit: false,
	audio: { codec: 'flac', lossless: true },
	availability: 'available',
	sortKeys: { title: title.toLowerCase(), artist: f.artist ?? '', album: f.album ?? '' }
});

// The server's fixture (`crates/server/src/api/tracks/tests.rs`), whose orders its tests pin:
// Abbey Road on two discs with one track unnumbered, Kid A, and an untagged file.
const abbey = { artist: 'beatles', album: 'abbey road', albumId: '101', date: '1969' };
const library = [
	track('1001', 'Come Together', { ...abbey, disc: 1, n: 1, duration: 259_000_000 }),
	track('1002', 'Something', { ...abbey, disc: 1, n: 2, duration: 182_000_000 }),
	track('1003', 'Her Majesty', { ...abbey, disc: 1, duration: 23_000_000 }),
	track('1004', 'Bonus', { ...abbey, disc: 2, n: 1, duration: 182_000_000 }),
	track('1005', 'Idioteque', {
		artist: 'radiohead',
		album: 'kid a',
		albumId: '102',
		disc: 1,
		n: 8,
		date: '2000',
		duration: 251_000_000
	}),
	track('1006', 'untitled', { albumId: '104' })
];

const titles = (sort: TrackSort, tracks = library) => sortTracks(tracks, sort).map((t) => t.title);

describe('nextSort', () => {
	it('goes ascending, descending, then unsorted', () => {
		const asc = nextSort(null, 'artist');
		expect(asc).toEqual({ field: 'artist', order: 'asc' });
		const desc = nextSort(asc, 'artist');
		expect(desc).toEqual({ field: 'artist', order: 'desc' });
		expect(nextSort(desc, 'artist')).toBeNull();
	});

	it('starts another column over at ascending', () => {
		expect(nextSort({ field: 'artist', order: 'desc' }, 'year')).toEqual({
			field: 'year',
			order: 'asc'
		});
	});
});

describe('serverSort', () => {
	it('names each sort as listTracks does', () => {
		expect(serverSort({ field: 'title', order: 'asc' })).toEqual({ sort: 'name', order: 'asc' });
		expect(serverSort({ field: 'year', order: 'desc' })).toEqual({
			sort: 'releaseDate',
			order: 'desc'
		});
		expect(serverSort({ field: 'duration', order: 'asc' })?.sort).toBe('duration');
	});

	it('leaves unsorted, and date added to a playlist, to the list', () => {
		expect(serverSort(null)).toBeNull();
		expect(serverSort({ field: 'added', order: 'asc' })).toBeNull();
	});
});

describe('compareKeys', () => {
	it('orders by code point, as the server orders UTF-8 bytes', () => {
		// U+1F3B5 is two UTF-16 units starting 0xD83C, below U+FFFD's one; by code point it is above.
		expect(compareKeys('\u{1F3B5}', '�')).toBeGreaterThan(0);
		expect(compareKeys('�', '\u{1F3B5}')).toBeLessThan(0);
		expect(compareKeys('b', 'é')).toBeLessThan(0);
	});

	it('puts a prefix first', () => {
		expect(compareKeys('abbey', 'abbey road')).toBeLessThan(0);
		expect(compareKeys('abbey road', 'abbey road')).toBe(0);
	});
});

describe('sortTracks', () => {
	it("orders the server's fixture as the server does", () => {
		expect(titles({ field: 'title', order: 'asc' })).toEqual([
			'Bonus',
			'Come Together',
			'Her Majesty',
			'Idioteque',
			'Something',
			'untitled'
		]);
		expect(titles({ field: 'album', order: 'asc' })).toEqual([
			'Come Together',
			'Something',
			'Her Majesty',
			'Bonus',
			'Idioteque',
			'untitled'
		]);
		expect(titles({ field: 'year', order: 'desc' })).toEqual([
			'Idioteque',
			'Bonus',
			'Her Majesty',
			'Something',
			'Come Together',
			'untitled'
		]);
		expect(titles({ field: 'duration', order: 'asc' })).toEqual([
			'untitled',
			'Her Majesty',
			'Something',
			'Bonus',
			'Idioteque',
			'Come Together'
		]);
		expect(titles({ field: 'duration', order: 'desc' })).toEqual([
			'Come Together',
			'Idioteque',
			'Bonus',
			'Something',
			'Her Majesty',
			'untitled'
		]);
	});

	it('breaks artist ties by title', () => {
		expect(titles({ field: 'artist', order: 'asc' })).toEqual([
			'Bonus',
			'Come Together',
			'Her Majesty',
			'Something',
			'Idioteque',
			'untitled'
		]);
	});

	it('puts tracks with no value last in either direction', () => {
		expect(titles({ field: 'artist', order: 'desc' })).toEqual([
			'Idioteque',
			'Something',
			'Her Majesty',
			'Come Together',
			'Bonus',
			'untitled'
		]);
		const undated = [track('1', 'B'), track('2', 'A', { date: '2001' }), track('3', 'C')];
		expect(titles({ field: 'year', order: 'asc' }, undated)).toEqual(['A', 'B', 'C']);
		expect(titles({ field: 'year', order: 'desc' }, undated)).toEqual(['A', 'C', 'B']);
	});

	it('ignores leading articles and case through the sort keys', () => {
		const beatles = track('1', 'x', { artist: 'beatles' });
		const abba = track('2', 'y', { artist: 'abba' });
		expect(titles({ field: 'artist', order: 'asc' }, [beatles, abba])).toEqual(['y', 'x']);
	});

	it('leaves a list unsorted as it is', () => {
		expect(sortTracks(library, null)).toBe(library);
	});

	it('does not change the list it sorts', () => {
		const before = [...library];
		sortTracks(library, { field: 'title', order: 'desc' });
		expect(library).toEqual(before);
	});
});

describe('sortRows', () => {
	// A playlist's rows: a track can be there twice, and each row has its own date added.
	const a = track('1', 'A');
	const b = track('2', 'B');
	const rows = [
		{ track: b, addedAt: '2026-03-01T00:00:00.000Z', position: 0 },
		{ track: a, addedAt: '2026-01-01T00:00:00.000Z', position: 1 },
		{ track: b, addedAt: '2026-02-01T00:00:00.000Z', position: 2 },
		{ track: a, addedAt: '2026-02-01T00:00:00.000Z', position: 3 }
	];
	const positions = (sort: TrackSort) =>
		sortRows(
			rows,
			sort,
			(r) => r.track,
			(r) => r.addedAt
		).map((r) => r.position);

	it('keeps a track held twice in the order it had', () => {
		expect(positions({ field: 'title', order: 'asc' })).toEqual([1, 3, 0, 2]);
		expect(positions({ field: 'title', order: 'desc' })).toEqual([0, 2, 1, 3]);
	});

	it('sorts by date added to the playlist, ties in the playlist order', () => {
		expect(positions({ field: 'added', order: 'asc' })).toEqual([1, 2, 3, 0]);
		expect(positions({ field: 'added', order: 'desc' })).toEqual([0, 2, 3, 1]);
	});
});
