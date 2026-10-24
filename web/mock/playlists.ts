// The user's playlists, kept in memory: a few generated from the library to start with, which
// the client can then create, edit, and delete (`api/openapi.yaml`, Playlists). They are the
// user's data, not the library's, so the scanner's database is never written, and they reset
// whenever the dev server restarts.

import { randomUUID } from 'node:crypto';
import type {
	Playable,
	Playlist,
	PlaylistItemPage,
	PlaylistOp,
	PlaylistPage,
	Problem,
	TrackSummary
} from '../src/lib/api/types.ts';
import { applyOps } from '../src/lib/playlist-ops.ts';
import { type Catalog, paging, slice } from './catalog.ts';
import { type Db, iso, toKey } from './db.ts';

/** What the store needs from a library: the scanner's database, or a stand-in in specs. */
export interface PlaylistLibrary {
	/** Playlists to start with. */
	generated(lib: bigint): Generated[];
	/** The tracks a track, album, or artist stands for, in order; undefined if there is no such thing. */
	expand(
		lib: bigint,
		ref: { type: 'track' | 'album' | 'artist'; id: string }
	): bigint[] | undefined;
	/** A track's summary; undefined once the library no longer has it. */
	track(lib: bigint, trackId: bigint): TrackSummary | undefined;
}

export type Generated = {
	id: string;
	title: string;
	description: string;
	createdAt: string;
	trackIds: bigint[];
};

type Item = { trackId: bigint; addedAt: string };
type Entry = { playlist: Playlist; items: Item[] };

/** A refusal, answered as a Problem. */
export class PlaylistError extends Error {
	constructor(
		readonly status: number,
		readonly code: Problem['code'],
		message: string,
		readonly extra: Partial<Problem> = {}
	) {
		super(message);
	}
}

const notFound = () => new PlaylistError(404, 'not_found', 'No such playlist.');

/** `validation_failed`, whose `detail` ends by pointing at the part of the body that is wrong. */
const invalid = (field: string, message: string) =>
	new PlaylistError(422, 'validation_failed', field ? `${message} (at ${field})` : message);

const isObject = (v: unknown): v is Record<string, unknown> =>
	typeof v === 'object' && v !== null && !Array.isArray(v);

export class PlaylistStore {
	#libraries = new Map<bigint, Map<string, Entry>>();
	/** Responses by `Idempotency-Key`, so a retried write answers as it first did. */
	#replies = new Map<string, PlaylistResponse>();

	constructor(readonly library: PlaylistLibrary) {}

	/** Pinned first, then by `sort` (`requirements/playlists.md` §7). */
	list(lib: bigint, query: URLSearchParams): PlaylistPage {
		const by: Record<string, (a: Playlist, b: Playlist) => number> = {
			name: (a, b) => a.title.localeCompare(b.title),
			created: (a, b) => a.createdAt.localeCompare(b.createdAt),
			modified: (a, b) => a.modifiedAt.localeCompare(b.modifiedAt)
		};
		// playCount needs listening data, which the mock has none of; modified stands in.
		const compare = by[query.get('sort') ?? 'modified'] ?? by.modified;
		const sign = query.get('order') === 'desc' ? -1 : 1;
		const text = query.get('q')?.trim().toLowerCase() ?? '';
		const lists = this.all(lib)
			.filter((p) => p.title.toLowerCase().includes(text))
			.sort((a, b) => Number(b.pinned) - Number(a.pinned) || sign * compare(a, b));
		return slice(lists, paging(query));
	}

	all(lib: bigint): Playlist[] {
		return [...this.#entries(lib).values()].map((e) => e.playlist);
	}

	get(lib: bigint, playlistId: string): Playlist | undefined {
		return this.#entries(lib).get(playlistId)?.playlist;
	}

	/** In order, duplicates included: what a mosaic is made from. */
	trackIds(lib: bigint, playlistId: string): bigint[] | undefined {
		return this.#entries(lib)
			.get(playlistId)
			?.items.map((i) => i.trackId);
	}

	items(lib: bigint, playlistId: string, query: URLSearchParams): PlaylistItemPage | undefined {
		const entry = this.#entries(lib).get(playlistId);
		if (!entry) return undefined;
		const page = slice(entry.items, paging(query));
		return {
			...page,
			items: page.items.flatMap((i) => {
				const track = this.library.track(lib, i.trackId);
				return track ? [{ track, addedAt: i.addedAt }] : [];
			}),
			version: entry.playlist.version
		};
	}

	create(lib: bigint, body: unknown): Playlist {
		if (!isObject(body)) throw invalid('', 'The request body must be an object.');
		const { title, description, items, fromMix } = body;
		if (typeof title !== 'string' || !title.trim()) {
			throw invalid('/title', 'A playlist needs a title.');
		}
		if (description !== undefined && typeof description !== 'string') {
			throw invalid('/description', 'The description must be text.');
		}
		if (fromMix !== undefined) throw invalid('/fromMix', 'The mock has no mixes to save.');
		if (items !== undefined && !Array.isArray(items)) {
			throw invalid('/items', 'Items must be a list.');
		}
		const now = new Date().toISOString();
		const trackIds = (items ?? []).flatMap((p, i) => this.#expand(lib, p, `/items/${i}`));
		const entry: Entry = {
			playlist: {
				id: `pl-${randomUUID()}`,
				libraryId: String(lib),
				title,
				description: description ?? null,
				image: null,
				customImage: false,
				trackCount: 0,
				durationUs: 0,
				pinned: false,
				createdAt: now,
				modifiedAt: now,
				version: 1
			},
			items: trackIds.map((trackId) => ({ trackId, addedAt: now }))
		};
		this.#refresh(lib, entry);
		this.#entries(lib).set(entry.playlist.id, entry);
		return entry.playlist;
	}

	/** All of it against `baseVersion`, or none of it (`requirements/playlists.md` §4–§5). */
	edit(lib: bigint, playlistId: string, body: unknown): Playlist {
		const entry = this.#entries(lib).get(playlistId);
		if (!entry) throw notFound();
		if (!isObject(body)) throw invalid('', 'The request body must be an object.');
		const { baseVersion, title, description, pinned, ops } = body;
		if (!Number.isInteger(baseVersion)) {
			throw invalid('/baseVersion', 'An edit needs the version it was made against.');
		}
		if ([title, description, pinned, ops].every((v) => v === undefined)) {
			throw invalid('', 'The edit changes nothing.');
		}
		if (title !== undefined && (typeof title !== 'string' || !title.trim())) {
			throw invalid('/title', 'A playlist needs a title.');
		}
		if (description !== undefined && description !== null && typeof description !== 'string') {
			throw invalid('/description', 'The description must be text.');
		}
		if (pinned !== undefined && typeof pinned !== 'boolean') {
			throw invalid('/pinned', 'Pinned must be true or false.');
		}
		if (ops !== undefined) this.#checkOps(lib, ops);

		const current = entry.playlist.version;
		if (baseVersion !== current) {
			throw new PlaylistError(
				409,
				'version_conflict',
				'The playlist changed somewhere else, so this edit was not applied.',
				{ currentVersion: current }
			);
		}

		let items = entry.items;
		if (ops !== undefined) {
			const now = new Date().toISOString();
			try {
				items = applyOps(items, ops as PlaylistOp[], (added) =>
					added
						.flatMap((p) => this.#expand(lib, p, ''))
						.map((trackId) => ({ trackId, addedAt: now }))
				);
			} catch (e) {
				if (e instanceof RangeError) throw invalid('/ops', `${e.message}.`);
				throw e;
			}
		}
		entry.items = items;
		entry.playlist = {
			...entry.playlist,
			...(title !== undefined && { title: title as string }),
			...(description !== undefined && { description: description as string | null }),
			...(pinned !== undefined && { pinned: pinned as boolean }),
			version: current + 1,
			modifiedAt: new Date().toISOString()
		};
		this.#refresh(lib, entry);
		return entry.playlist;
	}

	/** Whether there was such a playlist. */
	delete(lib: bigint, playlistId: string): boolean {
		return this.#entries(lib).delete(playlistId);
	}

	/** The tracks `items` stand for that the playlist already has, once each, in request order. */
	duplicates(lib: bigint, playlistId: string, body: unknown): { trackIds: string[] } {
		const entry = this.#entries(lib).get(playlistId);
		if (!entry) throw notFound();
		if (!isObject(body) || !Array.isArray(body.items)) {
			throw invalid('/items', 'Items must be a list.');
		}
		const have = new Set(entry.items.map((i) => i.trackId));
		const asked = body.items.flatMap((p, i) => this.#expand(lib, p, `/items/${i}`));
		return { trackIds: [...new Set(asked.filter((t) => have.has(t)))].map(String) };
	}

	/** The response a write with this `Idempotency-Key` gave, if there was one. */
	reply(key: string): PlaylistResponse | undefined {
		return this.#replies.get(key);
	}

	remember(key: string, response: PlaylistResponse) {
		this.#replies.set(key, response);
	}

	#entries(lib: bigint): Map<string, Entry> {
		let entries = this.#libraries.get(lib);
		if (!entries) {
			entries = new Map();
			for (const g of this.library.generated(lib)) {
				const entry: Entry = {
					playlist: {
						id: g.id,
						libraryId: String(lib),
						title: g.title,
						description: g.description,
						image: null,
						customImage: false,
						trackCount: 0,
						durationUs: 0,
						pinned: false,
						createdAt: g.createdAt,
						modifiedAt: g.createdAt,
						version: 1
					},
					items: g.trackIds.map((trackId) => ({ trackId, addedAt: g.createdAt }))
				};
				this.#refresh(lib, entry);
				entries.set(g.id, entry);
			}
			this.#libraries.set(lib, entries);
		}
		return entries;
	}

	/** Totals and artwork, after the contents change. */
	#refresh(lib: bigint, entry: Entry) {
		const { playlist, items } = entry;
		playlist.trackCount = items.length;
		playlist.durationUs = items.reduce(
			(us, i) => us + (this.library.track(lib, i.trackId)?.durationUs ?? 0),
			0
		);
		// A mosaic of the albums inside, composed by `media.ts` (`requirements/playlists.md` §6).
		// Its ID carries the version, as an image's ID changes whenever the image does.
		playlist.image = items.length
			? { id: `mosaic-${playlist.version}-${playlist.id}`, placeholder: null }
			: null;
	}

	/** Every op's shape and items, so an edit is refused before any of it applies. */
	#checkOps(lib: bigint, ops: unknown) {
		if (!Array.isArray(ops) || ops.length === 0) throw invalid('/ops', 'Ops must be a list.');
		ops.forEach((op, i) => {
			const field = `/ops/${i}`;
			if (!isObject(op)) throw invalid(field, 'An op must be an object.');
			if (op.op === 'add') {
				if (!Array.isArray(op.items) || op.items.length === 0) {
					throw invalid(`${field}/items`, 'An add needs items.');
				}
				op.items.forEach((p, j) => this.#expand(lib, p, `${field}/items/${j}`));
			} else if (op.op === 'remove' || op.op === 'move') {
				if (!Array.isArray(op.positions)) {
					throw invalid(`${field}/positions`, 'Positions must be a list.');
				}
			} else {
				throw invalid(`${field}/op`, 'An op is add, remove, or move.');
			}
		});
	}

	/** The tracks something added stands for. Playlists are the store's own, the rest the library's. */
	#expand(lib: bigint, p: unknown, field: string): bigint[] {
		const ref = p as Playable;
		if (!isObject(p) || typeof p.type !== 'string') throw invalid(field, 'Not something playable.');
		if (ref.type === 'external') throw invalid(field, 'The mock has no external sources.');
		let ids: bigint[] | undefined;
		if (ref.type === 'playlist') ids = this.trackIds(lib, String(ref.id));
		else if (ref.type === 'track' || ref.type === 'album' || ref.type === 'artist') {
			ids = this.library.expand(lib, { type: ref.type, id: String(ref.id) });
		} else throw invalid(field, `A ${ref.type} can't be added to a playlist.`);
		if (!ids) throw invalid(field, `This library has no such ${ref.type}.`);
		return ids;
	}
}

export type PlaylistResponse = { status: number; body?: unknown };

/**
 * `/playlists…` below a library the caller has already found, for any transport: the Vite
 * middleware calls it, and so can tests. Undefined for a path that is not a playlist's.
 */
export function playlistRoute(
	store: PlaylistStore,
	lib: bigint,
	method: string,
	path: string,
	body: Uint8Array,
	idempotencyKey?: string
): PlaylistResponse | undefined {
	const url = new URL(path, 'http://mock');
	const m = /^\/playlists(?:\/([^/]+)(?:\/(items|duplicates))?)?$/.exec(url.pathname);
	if (!m) return undefined;
	const [, id, sub] = m.map((s) => (s === undefined ? s : decodeURIComponent(s)));
	const verb = method === 'HEAD' ? 'GET' : method;
	// Creating and editing take a key (`api/openapi.yaml`, `IdempotencyKey`).
	const keyed =
		idempotencyKey && ((!id && verb === 'POST') || (id && !sub && verb === 'PATCH'))
			? `${lib}/${idempotencyKey}`
			: undefined;
	const replay = keyed && store.reply(keyed);
	if (replay) return replay;

	const json = () => {
		try {
			return JSON.parse(new TextDecoder().decode(body));
		} catch {
			throw invalid('', 'The request body is not JSON.');
		}
	};
	const found = (v: unknown) => (v === undefined ? fail(notFound()) : { status: 200, body: v });
	let out: PlaylistResponse;
	try {
		if (!id && verb === 'GET') out = { status: 200, body: store.list(lib, url.searchParams) };
		else if (!id && verb === 'POST') out = { status: 201, body: store.create(lib, json()) };
		else if (id && !sub && verb === 'GET') out = found(store.get(lib, id));
		else if (id && !sub && verb === 'PATCH')
			out = { status: 200, body: store.edit(lib, id, json()) };
		else if (id && !sub && verb === 'DELETE') {
			if (!store.delete(lib, id)) throw notFound();
			out = { status: 204 };
		} else if (sub === 'items' && verb === 'GET') {
			out = found(store.items(lib, id, url.searchParams));
		} else if (sub === 'duplicates' && verb === 'POST') {
			out = { status: 200, body: store.duplicates(lib, id, json()) };
		} else {
			out = fail(new PlaylistError(405, 'method_not_allowed', 'Not supported.'));
		}
	} catch (e) {
		if (!(e instanceof PlaylistError)) throw e;
		out = fail(e);
	}
	if (keyed) store.remember(keyed, out);
	return out;
}

function fail(e: PlaylistError): PlaylistResponse {
	const titles: Record<number, string> = {
		404: 'Not Found',
		405: 'Method Not Allowed',
		409: 'Conflict',
		422: 'Unprocessable Content'
	};
	const problem: Problem = {
		type: 'about:blank',
		title: titles[e.status] ?? 'Error',
		status: e.status,
		code: e.code,
		detail: e.message,
		...e.extra
	};
	return { status: e.status, body: problem };
}

// ───────────────────────────── From the scanner's database ─────────────────────────────

/** The library as the scanner stored it. */
export function dbLibrary(db: Db, catalog: Catalog): PlaylistLibrary {
	return {
		generated: (lib) => generated(db, lib),
		expand(lib, ref) {
			const key = toKey(ref.id);
			if (key === null) return undefined;
			const albumOrder = 'ORDER BY album_sort_key, album_id, disc_number, track_number, id';
			const rows =
				ref.type === 'track'
					? db.all('SELECT id FROM tracks WHERE library_id = ? AND id = ?', lib, key)
					: ref.type === 'album'
						? db.all(
								`SELECT id FROM tracks WHERE library_id = ? AND album_id = ? ${albumOrder}`,
								lib,
								key
							)
						: db.all(
								`SELECT id FROM tracks WHERE library_id = ?
								 AND (album_id IN (SELECT album_id FROM album_artists WHERE artist_id = ?)
								      OR id IN (SELECT track_id FROM track_artists WHERE artist_id = ?))
								 ${albumOrder}`,
								lib,
								key,
								key
							);
			return rows.length ? rows.map((r) => r.id as bigint) : undefined;
		},
		track: (lib, trackId) => catalog.tracksByIds(lib, [trackId])[0]
	};
}

type Recipe = { id: string; title: string; description: string; sql: string; params?: bigint[] };

/** A few playlists derived from the library, so there is something to see before making any. */
function generated(db: Db, lib: bigint): Generated[] {
	const recipes: Recipe[] = [
		{
			id: 'pl-recent',
			title: 'Recently Added',
			description: 'The newest arrivals in your library.',
			sql: 'SELECT id FROM tracks WHERE library_id = ? AND missing_since IS NULL ORDER BY added_at DESC, id LIMIT 25'
		},
		{
			id: 'pl-long',
			title: 'Long Listens',
			description: 'The longest tracks you have.',
			sql: 'SELECT id FROM tracks WHERE library_id = ? AND missing_since IS NULL ORDER BY duration_us DESC, id LIMIT 25'
		}
	];
	const genres = db.all(
		"SELECT id, name FROM tags WHERE library_id = ? AND type = 'genre' ORDER BY track_count DESC, sort_key LIMIT 2",
		lib
	);
	for (const g of genres) {
		recipes.push({
			id: `pl-genre-${g.id}`,
			title: `${g.name} Mix`,
			description: `A run through your ${g.name} collection.`,
			sql: `SELECT t.id FROM track_tags x JOIN tracks t ON t.id = x.track_id
			      WHERE t.library_id = ? AND x.tag_id = ? ORDER BY t.album_sort_key, t.disc_number, t.track_number, t.id LIMIT 50`,
			params: [g.id as bigint]
		});
	}

	const createdAt = iso(
		db.get('SELECT MIN(created_at) AS at FROM library_roots WHERE library_id = ?', lib)?.at ?? 0
	);
	return recipes.flatMap((r) => {
		const trackIds = db.all(r.sql, lib, ...(r.params ?? [])).map((row) => row.id as bigint);
		const { id, title, description } = r;
		return trackIds.length ? [{ id, title, description, createdAt, trackIds }] : [];
	});
}
