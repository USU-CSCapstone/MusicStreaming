// The mock's routes: the read-only subset of `api/openapi.yaml` the web client
// uses, answered from the scanner's database, plus plugin administration (`plugins.ts`).

import type { IncomingMessage, ServerResponse } from 'node:http';
import { join } from 'node:path';
import type {
	Device,
	Lyrics,
	Problem,
	SearchResponse,
	SearchResult,
	SearchSectionType,
	User
} from '../src/lib/api/types.ts';
import { Catalog, matchRank, paging, slice } from './catalog.ts';
import { Db, toKey } from './db.ts';
import { image, imageSize, playbackInfo, streamAudio, waveform } from './media.ts';
import { playlist, playlists } from './playlists.ts';
import { PluginStore, adminRoute } from './plugins.ts';

type Ctx = {
	params: string[];
	lib: bigint;
	query: URLSearchParams;
	req: IncomingMessage;
	res: ServerResponse;
};
/** A JSON body, `undefined` for 404, or `null` when the handler wrote the response itself. */
type Handler = (c: Ctx) => unknown | Promise<unknown>;

export function createApi(dataDir: string) {
	const db = new Db(join(dataDir, 'state/jewelcase.db'));
	const catalog = new Catalog(db);
	// Installed plugins sit beside the scanner's state, never in the music library.
	const plugins = new PluginStore(join(dataDir, 'mock-plugins'), () =>
		catalog.libraries().map((l) => l.id)
	);

	// Paths under /libraries/{libraryId}; the library is resolved before the handler runs,
	// so nothing is reachable in a library that does not exist.
	const scoped: [RegExp, Handler][] = [
		[/^$/, ({ lib }) => catalog.library(lib) ?? undefined],
		[/^\/albums$/, ({ lib, query }) => catalog.listAlbums(lib, query)],
		[/^\/albums\/([^/]+)$/, ({ lib, params }) => withKey(params[0], (k) => catalog.album(lib, k))],
		[/^\/tracks$/, ({ lib, query }) => catalog.listTracks(lib, query)],
		[
			/^\/tracks\/([^/]+)$/,
			({ lib, params }) => withKey(params[0], (k) => catalog.tracksByIds(lib, [k])[0])
		],
		[/^\/artists$/, ({ lib, query }) => catalog.listArtists(lib, query)],
		[
			/^\/artists\/([^/]+)$/,
			({ lib, params }) => withKey(params[0], (k) => catalog.artist(lib, k))
		],
		[
			/^\/playlists$/,
			({ lib, query }) =>
				slice(
					playlists(db, lib).map((p) => p.playlist),
					paging(query)
				)
		],
		[/^\/playlists\/([^/]+)$/, ({ lib, params }) => playlist(db, lib, params[0])?.playlist],
		[
			/^\/playlists\/([^/]+)\/items$/,
			({ lib, params, query }) => {
				const list = playlist(db, lib, params[0]);
				if (!list) return undefined;
				const items = catalog.tracksByIds(lib, list.trackIds).map((track, i) => ({
					itemId: `${list.playlist.id}-${i}`,
					track,
					addedAt: list.playlist.createdAt
				}));
				return slice(items, paging(query));
			}
		],
		[/^\/search$/, ({ lib, query }) => search(lib, query)],
		[
			/^\/tracks\/([^/]+)\/waveform$/,
			({ lib, params, res }) =>
				withKey(params[0], (k) => {
					if (!catalog.trackFile(lib, k)) return undefined;
					const w = waveform(catalog, lib, k);
					if (w) return w;
					res.statusCode = 204; // Not analyzed yet; the client shows a plain bar.
					res.end();
					return null;
				})
		],
		[/^\/tracks\/([^/]+)\/lyrics$/, ({ lib, params }) => withKey(params[0], (k) => lyrics(lib, k))],
		[
			/^\/tracks\/([^/]+)\/playback$/,
			({ lib, params }) =>
				withKey(params[0], (k) => mapRow(catalog.trackFile(lib, k), playbackInfo))
		],
		[
			/^\/tracks\/([^/]+)\/audio$/,
			({ lib, params, req, res }) =>
				withKey(params[0], (k) => {
					const track = catalog.trackFile(lib, k);
					if (!track) return undefined;
					streamAudio(track, req, res);
					return null;
				})
		],
		[
			/^\/images\/([^/]+)$/,
			async ({ lib, params, query, res }) => {
				const jpeg = await image(db, lib, params[0], imageSize(query.get('size')));
				if (!jpeg) return undefined;
				res.setHeader('Content-Type', 'image/jpeg');
				// Image IDs change whenever the image does, so they cache forever.
				res.setHeader('Cache-Control', 'public, max-age=31536000, immutable');
				res.end(jpeg);
				return null;
			}
		]
	];

	/** What the scanner stored from embedded tags or a `.lrc`/`.txt` beside the track (`requirements/tracks.md` §5). */
	function lyrics(lib: bigint, trackId: bigint): Lyrics | undefined {
		if (!catalog.trackFile(lib, trackId)) return undefined;
		const row = db.get(
			'SELECT plain, synced FROM track_lyrics WHERE library_id = ? AND track_id = ?',
			lib,
			trackId
		);
		if (!row) return { kind: 'none', lines: null, plain: null };
		if (row.synced !== null) {
			// The stored text of synced lyrics is the LRC itself, not a plain version.
			return { kind: 'synced', lines: JSON.parse(String(row.synced)), plain: null };
		}
		return { kind: 'plain', lines: null, plain: String(row.plain) };
	}

	function search(lib: bigint, query: URLSearchParams): SearchResponse | undefined {
		const text = query.get('q')?.trim() ?? '';
		if (!text) return undefined;
		const limit = Math.min(50, Math.max(1, Number(query.get('sectionLimit')) || 5));
		const wanted = query.get('types')?.split(',') ?? ['tracks', 'albums', 'artists', 'playlists'];
		const sections: {
			type: SearchSectionType;
			items: SearchResult[];
			total: number;
			rank: number;
		}[] = [];
		const add = (
			type: SearchSectionType,
			items: SearchResult[],
			total: number,
			best: string | null
		) => {
			if (wanted.includes(type) && items.length)
				sections.push({ type, items, total, rank: matchRank(best, text) });
		};
		const tracks = catalog.searchTracks(lib, text, limit);
		add(
			'tracks',
			tracks.items.map((track) => ({ type: 'track', track })),
			tracks.total,
			tracks.items[0]?.title ?? null
		);
		const albums = catalog.searchAlbums(lib, text, limit);
		add(
			'albums',
			albums.items.map((album) => ({ type: 'album', album })),
			albums.total,
			albums.items[0]?.title ?? null
		);
		const artists = catalog.searchArtists(lib, text, limit);
		add(
			'artists',
			artists.items.map((artist) => ({ type: 'artist', artist })),
			artists.total,
			artists.items[0]?.name ?? null
		);
		const lists = playlists(db, lib)
			.map((p) => p.playlist)
			.filter((p) => matchRank(p.title, text) >= 0)
			.sort((a, b) => matchRank(b.title, text) - matchRank(a.title, text));
		add(
			'playlists',
			lists.slice(0, limit).map((playlist) => ({ type: 'playlist', playlist })),
			lists.length,
			lists[0]?.title ?? null
		);

		// Sections by best match; the stable sort keeps tracks, albums, artists on ties.
		sections.sort((a, b) => b.rank - a.rank);
		const library = catalog.library(lib);
		return {
			query: query.get('q') ?? '',
			top: sections[0]?.items[0] ?? null,
			sections: sections.map(({ type, items, total }) => ({ type, items, total })),
			nearMisses: [],
			libraryEmpty: (library?.trackCount ?? 0) === 0
		};
	}

	async function route(
		req: IncomingMessage,
		res: ServerResponse,
		path: string,
		query: URLSearchParams
	) {
		if (path === '/health') return { status: 'ok' };
		// The mock has no accounts: it is always set up, and everyone is this one user.
		if (path === '/server') return { version: 'mock', apiVersion: '1', setupRequired: false };
		if (path === '/me') return MOCK_USER;
		// Nothing to connect: the mock's plugins act for no one in particular.
		if (path === '/me/plugins') return { items: [] };
		if (path === '/libraries') return { items: catalog.libraries() };
		const m = /^\/libraries\/([^/]+)(.*)$/.exec(path);
		const lib = toKey(m?.[1]);
		if (!m || lib === null || !catalog.library(lib)) return undefined;
		for (const [pattern, handler] of scoped) {
			const match = pattern.exec(m[2]);
			if (match)
				return handler({ params: match.slice(1).map(decodeURIComponent), lib, query, req, res });
		}
		return undefined;
	}

	/** Connect-style middleware, mounted at `/api/v1`. */
	return async (req: IncomingMessage, res: ServerResponse) => {
		const url = new URL(req.url ?? '/', 'http://mock');
		const path = url.pathname.replace(/\/$/, '');
		// Logging in and out always succeed, so the login page and Log out work.
		// Plays are taken and forgotten: the mock keeps no history.
		if (req.method === 'POST' && (path === '/auth/logout' || path === '/me/plays')) {
			res.statusCode = 204;
			return res.end();
		}
		if (req.method === 'POST' && path === '/auth/login') {
			res.setHeader('Content-Type', 'application/json');
			return res.end(JSON.stringify({ token: 'mock', user: MOCK_USER, device: MOCK_DEVICE }));
		}
		if (path.startsWith('/admin/plugins')) {
			try {
				const out = await adminRoute(
					plugins,
					req.method ?? 'GET',
					path + url.search,
					req.headers['content-type'] ?? '',
					await readBody(req)
				);
				if (!out) return problem(res, 404, 'Not Found', 'not_found');
				res.statusCode = out.status;
				if (out.body === undefined) return res.end();
				res.setHeader(
					'Content-Type',
					out.status < 400 ? 'application/json' : 'application/problem+json'
				);
				return res.end(JSON.stringify(out.body));
			} catch (e) {
				console.error('[mock api]', e);
				res.statusCode = 500;
				return res.end(String(e));
			}
		}
		if (req.method !== 'GET' && req.method !== 'HEAD') {
			return problem(res, 405, 'Method Not Allowed', 'method_not_allowed', 'The mock API is read-only.');
		}
		try {
			const body = await route(req, res, path, url.searchParams);
			if (body === null) return; // The handler wrote the response.
			if (body === undefined) return problem(res, 404, 'Not Found', 'not_found');
			res.setHeader('Content-Type', 'application/json');
			res.end(JSON.stringify(body));
		} catch (e) {
			// A mock bug, not an API response; the spec has no Problem code for it.
			console.error('[mock api]', e);
			if (!res.headersSent) res.statusCode = 500;
			res.end(String(e));
		}
	};
}

const MOCK_USER: User = {
	id: '1',
	username: 'mock',
	displayName: 'Mock User',
	role: 'owner',
	hasAvatar: false
};

const MOCK_DEVICE: Device = {
	id: '1',
	name: 'Mock device',
	type: 'desktop',
	firstSeenAt: '2026-01-01T00:00:00.000Z',
	lastSeenAt: '2026-01-01T00:00:00.000Z',
	connected: false
};

/** The request body, up to a little over the 50 MB plugin limit. */
async function readBody(req: IncomingMessage): Promise<Uint8Array> {
	const chunks: Buffer[] = [];
	let size = 0;
	for await (const chunk of req) {
		size += (chunk as Buffer).length;
		if (size > 51 * 1024 * 1024) break;
		chunks.push(chunk as Buffer);
	}
	return new Uint8Array(Buffer.concat(chunks));
}

function withKey<T>(s: string, f: (k: bigint) => T): T | undefined {
	const k = toKey(s);
	return k === null ? undefined : f(k);
}

function mapRow<R, T>(row: R | undefined, f: (r: R) => T): T | undefined {
	return row === undefined ? undefined : f(row);
}

function problem(
	res: ServerResponse,
	status: number,
	title: string,
	code: Problem['code'],
	detail?: string
) {
	const body: Problem = { type: 'about:blank', title, status, code, ...(detail ? { detail } : {}) };
	res.statusCode = status;
	res.setHeader('Content-Type', 'application/problem+json');
	res.end(JSON.stringify(body));
}
