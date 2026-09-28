// Scanner rows → API entities, following `api/openapi.yaml`. Every query is
// scoped by library_id (`requirements/general.md` §3.6).

import type { SQLInputValue } from 'node:sqlite';
import type {
	Album,
	AlbumSummary,
	Artist,
	ArtistCredit,
	ArtistSummary,
	ImageRef,
	Library,
	Page,
	TrackSummary
} from '../src/lib/api/types.ts';
import { type Db, type Row, id, iso, num, numOrNull, str, toKey } from './db.ts';

/** A filter ID as a key; an ID that cannot exist matches nothing rather than erroring. */
function keyOrNone(s: string): bigint {
	return toKey(s) ?? -1n;
}

// ───────────────────────────── Paging ─────────────────────────────

export type Paging = { limit: number; offset: number };

export function paging(q: URLSearchParams): Paging {
	const limit = Math.min(1000, Math.max(1, Number(q.get('limit') ?? 100) || 100));
	const cursor = q.get('cursor');
	const offset = cursor ? Number(Buffer.from(cursor, 'base64url').toString().slice(2)) || 0 : 0;
	return { limit, offset };
}

export function page<T>(items: T[], total: number, { limit, offset }: Paging): Page<T> {
	const next = offset + limit;
	return {
		items,
		total,
		nextCursor: next < total ? Buffer.from(`o:${next}`).toString('base64url') : null
	};
}

/** Offset paging over an in-memory list. */
export function slice<T>(all: T[], p: Paging): Page<T> {
	return page(all.slice(p.offset, p.offset + p.limit), all.length, p);
}

function orderBy(cols: string[], q: URLSearchParams): string {
	const dir = q.get('order') === 'desc' ? ' DESC' : '';
	return cols.map((c) => c + dir).join(', ');
}

/** SQL `LIKE` pattern matching `text` anywhere, with wildcards in `text` escaped. */
export function likeAnywhere(text: string): string {
	return `%${text.replace(/[\\%_]/g, (c) => `\\${c}`)}%`;
}

// ───────────────────────────── Shared pieces ─────────────────────────────

export function imageRef(imageId: unknown): ImageRef | null {
	// `placeholder` is draft in the API and NULL in the scanner until the image job exists.
	return imageId === null || imageId === undefined ? null : { id: id(imageId), placeholder: '' };
}

export class Catalog {
	constructor(readonly db: Db) {}

	// ───────────────────────────── Libraries ─────────────────────────────

	#library(row: Row): Library {
		const scanning = this.db.get(
			"SELECT 1 FROM scans WHERE library_id = ? AND state IN ('queued', 'running') LIMIT 1",
			row.id as bigint
		);
		return {
			id: id(row.id),
			name: String(row.name),
			trackCount: num(row.track_count),
			albumCount: num(row.album_count),
			artistCount: num(row.artist_count),
			durationUs: num(row.duration_us),
			scanning: scanning !== undefined
		};
	}

	libraries(): Library[] {
		return this.db.all('SELECT * FROM libraries ORDER BY name, id').map((r) => this.#library(r));
	}

	library(lib: bigint): Library | null {
		const row = this.db.get('SELECT * FROM libraries WHERE id = ?', lib);
		return row ? this.#library(row) : null;
	}

	// ───────────────────────────── Credits ─────────────────────────────

	albumArtists(albumId: bigint): ArtistCredit[] {
		return this.db
			.all(
				`SELECT a.id, a.name FROM album_artists aa JOIN artists a ON a.id = aa.artist_id
				 WHERE aa.album_id = ? ORDER BY aa.position`,
				albumId
			)
			.map((r) => ({ id: id(r.id), name: str(r.name) }));
	}

	trackArtists(trackId: bigint): ArtistCredit[] {
		return this.db
			.all(
				`SELECT a.id, a.name FROM track_artists ta JOIN artists a ON a.id = ta.artist_id
				 WHERE ta.track_id = ? ORDER BY ta.position`,
				trackId
			)
			.map((r) => ({ id: id(r.id), name: str(r.name) }));
	}

	#genres(sql: string, key: bigint) {
		return this.db.all(sql, key).map((r) => ({ id: id(r.id), name: String(r.name) }));
	}

	// ───────────────────────────── Albums ─────────────────────────────

	#albumSummary(row: Row): AlbumSummary {
		const albumId = row.id as bigint;
		// The API's trackTotal sums the tagged per-disc totals of the discs present.
		const totals = this.db.get(
			'SELECT SUM(track_total) AS total, COUNT(track_total) AS tagged FROM album_discs WHERE album_id = ?',
			albumId
		);
		return {
			id: id(albumId),
			title: str(row.title),
			artists: this.albumArtists(albumId),
			releaseDate: str(row.release_date),
			type: row.type as AlbumSummary['type'],
			genres: this.#genres(
				'SELECT t.id, t.name FROM album_tags x JOIN tags t ON t.id = x.tag_id WHERE x.album_id = ? ORDER BY t.sort_key',
				albumId
			),
			trackCount: num(row.track_count),
			trackTotal: totals && num(totals.tagged) > 0 ? num(totals.total) : null,
			discCount: num(row.disc_count),
			discTotal: numOrNull(row.disc_total),
			durationUs: num(row.duration_us),
			image: imageRef(row.image_id),
			availability: num(row.available_track_count) > 0 ? 'available' : 'missing',
			addedAt: iso(row.added_at)
		};
	}

	listAlbums(lib: bigint, q: URLSearchParams): Page<AlbumSummary> {
		const where = ['al.library_id = ?'];
		const params: SQLInputValue[] = [lib];
		const artist = q.get('artistId');
		if (artist) {
			const owned = 'al.id IN (SELECT album_id FROM album_artists WHERE artist_id = ?)';
			const credited = `al.id IN (SELECT t.album_id FROM tracks t JOIN track_artists ta ON ta.track_id = t.id
			                            WHERE ta.artist_id = ?)`;
			const key = keyOrNone(artist);
			switch (q.get('artistCredit')) {
				case 'owned':
					where.push(owned);
					params.push(key);
					break;
				case 'featured':
					where.push(credited, `NOT ${owned}`);
					params.push(key, key);
					break;
				default:
					where.push(`(${owned} OR ${credited})`);
					params.push(key, key);
			}
		}
		const types = q.get('type')?.split(',').filter(Boolean);
		if (types?.length) {
			where.push(`al.type IN (${types.map(() => '?').join(', ')})`);
			params.push(...types);
		}
		const availability = q.get('availability');
		if (availability === 'available') where.push('al.available_track_count > 0');
		if (availability === 'missing') where.push('al.available_track_count = 0');

		const sorts: Record<string, string[]> = {
			name: ['al.sort_key', 'al.id'],
			artist: ['al.artist_sort_key', 'al.sort_key', 'al.id'],
			dateAdded: ['al.added_at', 'al.id'],
			releaseDate: ['al.release_date', 'al.sort_key', 'al.id'],
			duration: ['al.duration_us', 'al.id']
		};
		// playCount and lastPlayed need personal data, which the mock does not have.
		const sort = sorts[q.get('sort') ?? 'name'] ?? sorts.name;
		const p = paging(q);
		const clause = where.join(' AND ');
		const total = num(
			this.db.get(`SELECT COUNT(*) AS n FROM albums al WHERE ${clause}`, ...params)?.n
		);
		const rows = this.db.all(
			`SELECT al.* FROM albums al WHERE ${clause} ORDER BY ${orderBy(sort, q)} LIMIT ? OFFSET ?`,
			...params,
			p.limit,
			p.offset
		);
		return page(
			rows.map((r) => this.#albumSummary(r)),
			total,
			p
		);
	}

	album(lib: bigint, albumId: bigint): Album | null {
		const row = this.db.get('SELECT * FROM albums WHERE library_id = ? AND id = ?', lib, albumId);
		if (!row) return null;
		const discs = this.db
			.all(
				'SELECT disc_number, track_count, track_total FROM album_discs WHERE album_id = ? ORDER BY disc_number',
				albumId
			)
			.map((d) => ({
				// Disc 0 is the scanner's "no disc tag"; the API calls that null.
				number: num(d.disc_number) === 0 ? null : num(d.disc_number),
				trackCount: num(d.track_count),
				trackTotal: numOrNull(d.track_total)
			}));
		return { ...this.#albumSummary(row), labels: JSON.parse(String(row.labels)), discs };
	}

	/** Album searches: `LIKE` over titles, best matches first. */
	searchAlbums(lib: bigint, text: string, limit: number): { items: AlbumSummary[]; total: number } {
		const [where, params, rank] = matchClause('al.title', text);
		const total = num(
			this.db.get(
				`SELECT COUNT(*) AS n FROM albums al WHERE al.library_id = ? AND ${where}`,
				lib,
				...params
			)?.n
		);
		const rows = this.db.all(
			`SELECT al.* FROM albums al WHERE al.library_id = ? AND ${where}
			 ORDER BY ${rank} DESC, al.sort_key, al.id LIMIT ?`,
			lib,
			...params,
			...rankParams(text),
			limit
		);
		return { items: rows.map((r) => this.#albumSummary(r)), total };
	}

	// ───────────────────────────── Tracks ─────────────────────────────

	#trackSelect = `SELECT t.*, al.title AS album_display_title, al.image_id AS album_image_id
	                FROM tracks t JOIN albums al ON al.id = t.album_id`;

	#trackSummary(row: Row): TrackSummary {
		const trackId = row.id as bigint;
		const albumId = row.album_id as bigint;
		const disc = num(row.disc_number);
		return {
			id: id(trackId),
			title: String(row.title),
			artists: this.trackArtists(trackId),
			album: {
				id: id(albumId),
				title: str(row.album_display_title),
				artists: this.albumArtists(albumId),
				image: imageRef(row.album_image_id)
			},
			discNumber: disc === 0 ? null : disc,
			trackNumber: numOrNull(row.track_number),
			durationUs: num(row.duration_us),
			releaseDate: str(row.release_date),
			genres: this.#genres(
				'SELECT t.id, t.name FROM track_tags x JOIN tags t ON t.id = x.tag_id WHERE x.track_id = ? ORDER BY t.sort_key',
				trackId
			),
			explicit: num(row.explicit) === 1,
			audio: { codec: String(row.codec), lossless: num(row.lossless) === 1 },
			availability: row.missing_since === null ? 'available' : 'missing',
			addedAt: iso(row.added_at)
		};
	}

	listTracks(lib: bigint, q: URLSearchParams): Page<TrackSummary> {
		const where = ['t.library_id = ?'];
		const params: SQLInputValue[] = [lib];
		const album = q.get('albumId');
		if (album) {
			where.push('t.album_id = ?');
			params.push(keyOrNone(album));
		}
		const artist = q.get('artistId');
		if (artist) {
			const key = keyOrNone(artist);
			const owned = 't.album_id IN (SELECT album_id FROM album_artists WHERE artist_id = ?)';
			const credited = 't.id IN (SELECT track_id FROM track_artists WHERE artist_id = ?)';
			switch (q.get('artistCredit')) {
				case 'owned':
					where.push(owned);
					params.push(key);
					break;
				case 'featured':
					where.push(credited, `NOT ${owned}`);
					params.push(key, key);
					break;
				default:
					where.push(`(${owned} OR ${credited})`);
					params.push(key, key);
			}
		}
		const availability = q.get('availability');
		if (availability === 'available') where.push('t.missing_since IS NULL');
		if (availability === 'missing') where.push('t.missing_since IS NOT NULL');

		const albumOrder = [
			't.album_sort_key',
			't.album_id',
			't.disc_number',
			't.track_number',
			't.id'
		];
		const sorts: Record<string, string[]> = {
			name: ['t.sort_key', 't.id'],
			artist: ['t.artist_sort_key', 't.sort_key', 't.id'],
			album: albumOrder,
			dateAdded: ['t.added_at', 't.id'],
			releaseDate: ['t.release_date', ...albumOrder]
		};
		// top, playCount, and lastPlayed need listening data; album order stands in.
		const sort = sorts[q.get('sort') ?? 'name'] ?? albumOrder;
		const p = paging(q);
		const clause = where.join(' AND ');
		const total = num(
			this.db.get(`SELECT COUNT(*) AS n FROM tracks t WHERE ${clause}`, ...params)?.n
		);
		const rows = this.db.all(
			`${this.#trackSelect} WHERE ${clause} ORDER BY ${orderBy(sort, q)} LIMIT ? OFFSET ?`,
			...params,
			p.limit,
			p.offset
		);
		return page(
			rows.map((r) => this.#trackSummary(r)),
			total,
			p
		);
	}

	/** Summaries for the given track IDs, in the order given. */
	tracksByIds(lib: bigint, ids: bigint[]): TrackSummary[] {
		return ids.flatMap((t) => {
			const row = this.db.get(`${this.#trackSelect} WHERE t.library_id = ? AND t.id = ?`, lib, t);
			return row ? [this.#trackSummary(row)] : [];
		});
	}

	/** The raw row, for media endpoints: the file path, codec, and loudness. */
	trackFile(lib: bigint, trackId: bigint): Row | undefined {
		return this.db.get(
			`SELECT t.*, r.path AS root_path, al.loudness_lufs AS album_lufs, al.peak_dbtp AS album_peak
			 FROM tracks t JOIN library_roots r ON r.id = t.root_id JOIN albums al ON al.id = t.album_id
			 WHERE t.library_id = ? AND t.id = ?`,
			lib,
			trackId
		);
	}

	searchTracks(lib: bigint, text: string, limit: number): { items: TrackSummary[]; total: number } {
		const [where, params, rank] = matchClause('t.title', text);
		const total = num(
			this.db.get(
				`SELECT COUNT(*) AS n FROM tracks t WHERE t.library_id = ? AND ${where}`,
				lib,
				...params
			)?.n
		);
		const rows = this.db.all(
			`${this.#trackSelect} WHERE t.library_id = ? AND ${where}
			 ORDER BY ${rank} DESC, t.sort_key, t.id LIMIT ?`,
			lib,
			...params,
			...rankParams(text),
			limit
		);
		return { items: rows.map((r) => this.#trackSummary(r)), total };
	}

	// ───────────────────────────── Artists ─────────────────────────────

	#artistSummary(row: Row): ArtistSummary {
		return {
			id: id(row.id),
			name: str(row.name),
			image: imageRef(row.image_id),
			albumCount: num(row.album_count),
			trackCount: num(row.track_count),
			addedAt: iso(row.added_at)
		};
	}

	listArtists(lib: bigint, q: URLSearchParams): Page<ArtistSummary> {
		const sorts: Record<string, string[]> = {
			name: ['sort_key', 'id'],
			dateAdded: ['added_at', 'id']
		};
		const sort = sorts[q.get('sort') ?? 'name'] ?? sorts.name;
		const where = ['library_id = ?'];
		const availability = q.get('availability');
		if (availability === 'available') where.push('available_track_count > 0');
		if (availability === 'missing') where.push('available_track_count = 0');
		const clause = where.join(' AND ');
		const p = paging(q);
		const total = num(this.db.get(`SELECT COUNT(*) AS n FROM artists WHERE ${clause}`, lib)?.n);
		const rows = this.db.all(
			`SELECT * FROM artists WHERE ${clause} ORDER BY ${orderBy(sort, q)} LIMIT ? OFFSET ?`,
			lib,
			p.limit,
			p.offset
		);
		return page(
			rows.map((r) => this.#artistSummary(r)),
			total,
			p
		);
	}

	artist(lib: bigint, artistId: bigint): Artist | null {
		const row = this.db.get('SELECT * FROM artists WHERE library_id = ? AND id = ?', lib, artistId);
		if (!row) return null;
		return {
			...this.#artistSummary(row),
			biography: str(row.biography),
			genres: this.#genres(
				'SELECT t.id, t.name FROM artist_tags x JOIN tags t ON t.id = x.tag_id WHERE x.artist_id = ? ORDER BY t.sort_key',
				artistId
			),
			appearanceCount: num(row.appearance_count)
		};
	}

	searchArtists(
		lib: bigint,
		text: string,
		limit: number
	): { items: ArtistSummary[]; total: number } {
		const [where, params, rank] = matchClause('name', text);
		const total = num(
			this.db.get(
				`SELECT COUNT(*) AS n FROM artists WHERE library_id = ? AND ${where}`,
				lib,
				...params
			)?.n
		);
		const rows = this.db.all(
			`SELECT * FROM artists WHERE library_id = ? AND ${where}
			 ORDER BY ${rank} DESC, sort_key, id LIMIT ?`,
			lib,
			...params,
			...rankParams(text),
			limit
		);
		return { items: rows.map((r) => this.#artistSummary(r)), total };
	}
}

// ───────────────────────────── Search matching ─────────────────────────────

/**
 * A case-insensitive substring match, plus a rank: exact, then prefix, then
 * word-prefix, then anywhere. A stand-in for the real search (`requirements/search.md` §3).
 */
function matchClause(col: string, text: string): [string, SQLInputValue[], string] {
	const rank = `CASE WHEN lower(${col}) = lower(?) THEN 3
	              WHEN ${col} LIKE ? ESCAPE '\\' THEN 2
	              WHEN ${col} LIKE ? ESCAPE '\\' THEN 1 ELSE 0 END`;
	return [`${col} LIKE ? ESCAPE '\\'`, [likeAnywhere(text)], rank];
}

function rankParams(text: string): SQLInputValue[] {
	const escaped = text.replace(/[\\%_]/g, (c) => `\\${c}`);
	return [text, `${escaped}%`, `% ${escaped}%`];
}

/** The same rank in JavaScript, to order search sections by their best match. */
export function matchRank(value: string | null, text: string): number {
	if (value === null) return -1;
	const v = value.toLowerCase();
	const t = text.toLowerCase();
	if (v === t) return 3;
	if (v.startsWith(t)) return 2;
	if (v.includes(` ${t}`)) return 1;
	return v.includes(t) ? 0 : -1;
}
