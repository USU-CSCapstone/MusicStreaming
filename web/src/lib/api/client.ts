// Typed calls to the public API (`api/openapi.yaml`). Pages load through here,
// so this is the one module that changes when catalog reads move to the worker
// (`design/general.md` §7.1).

import { base } from '$app/paths';
import type {
	Album,
	AlbumPage,
	Artist,
	ArtistPage,
	ImageRef,
	Library,
	PlaybackInfo,
	Playlist,
	PlaylistItemPage,
	PlaylistPage,
	SearchResponse,
	ServerInfo,
	Session,
	SetupRequest,
	TrackPage,
	Waveform
} from './types';

type Fetch = typeof fetch;
type Query = Record<string, string | number | undefined>;

export const API = `${base}/api/v1`;

export class ApiError extends Error {
	constructor(
		readonly status: number,
		readonly code: string | undefined,
		message: string
	) {
		super(message);
	}
}

function url(path: string, query: Query = {}): string {
	const params = new URLSearchParams();
	for (const [k, v] of Object.entries(query)) if (v !== undefined) params.set(k, String(v));
	const qs = params.toString();
	return `${API}${path}${qs ? `?${qs}` : ''}`;
}

/** The response's JSON, or its Problem as an `ApiError` whose message is safe to show. */
async function body<T>(res: Response): Promise<T> {
	if (!res.ok) {
		const problem = await res.json().catch(() => null);
		const message = problem?.detail ?? problem?.title ?? res.statusText;
		throw new ApiError(res.status, problem?.code, message);
	}
	return res.json();
}

const get = async <T>(fetch: Fetch, path: string, query?: Query): Promise<T> =>
	body(await fetch(url(path, query)));

/** A write. The header lets the server tell it from a cross-site form post (`cookieAuth`). */
const post = async <T>(fetch: Fetch, path: string, json: unknown): Promise<T> =>
	body(
		await fetch(url(path), {
			method: 'POST',
			headers: { 'Content-Type': 'application/json', 'X-Jewelcase-Client': 'web' },
			body: JSON.stringify(json)
		})
	);

export const getServerInfo = (f: Fetch) => get<ServerInfo>(f, '/server');

/** Creates the owner and logs this browser in: the server sets the session cookie. */
export const completeSetup = (f: Fetch, request: SetupRequest) =>
	post<Session>(f, '/setup', request);

const lib = (libraryId: string) => `/libraries/${encodeURIComponent(libraryId)}`;

export const listLibraries = (f: Fetch) => get<{ items: Library[] }>(f, '/libraries');

export const listAlbums = (f: Fetch, libraryId: string, query: Query = {}) =>
	get<AlbumPage>(f, `${lib(libraryId)}/albums`, query);

export const getAlbum = (f: Fetch, libraryId: string, albumId: string) =>
	get<Album>(f, `${lib(libraryId)}/albums/${encodeURIComponent(albumId)}`);

export const listTracks = (f: Fetch, libraryId: string, query: Query = {}) =>
	get<TrackPage>(f, `${lib(libraryId)}/tracks`, query);

export const listArtists = (f: Fetch, libraryId: string, query: Query = {}) =>
	get<ArtistPage>(f, `${lib(libraryId)}/artists`, query);

export const getArtist = (f: Fetch, libraryId: string, artistId: string) =>
	get<Artist>(f, `${lib(libraryId)}/artists/${encodeURIComponent(artistId)}`);

export const listPlaylists = (f: Fetch, libraryId: string, query: Query = {}) =>
	get<PlaylistPage>(f, `${lib(libraryId)}/playlists`, query);

export const getPlaylist = (f: Fetch, libraryId: string, playlistId: string) =>
	get<Playlist>(f, `${lib(libraryId)}/playlists/${encodeURIComponent(playlistId)}`);

export const listPlaylistItems = (
	f: Fetch,
	libraryId: string,
	playlistId: string,
	query: Query = {}
) =>
	get<PlaylistItemPage>(
		f,
		`${lib(libraryId)}/playlists/${encodeURIComponent(playlistId)}/items`,
		query
	);

export const search = (f: Fetch, libraryId: string, q: string, query: Query = {}) =>
	get<SearchResponse>(f, `${lib(libraryId)}/search`, { q, ...query });

// `original` until streaming-quality settings exist (`requirements/users.md` §6).
export const getPlaybackInfo = (f: Fetch, libraryId: string, trackId: string) =>
	get<PlaybackInfo>(f, `${lib(libraryId)}/tracks/${encodeURIComponent(trackId)}/playback`, {
		purpose: 'stream',
		quality: 'original'
	});

/** The track's waveform, or null while it has not been analyzed (`204`). */
export async function getWaveform(
	f: Fetch,
	libraryId: string,
	trackId: string
): Promise<Waveform | null> {
	const res = await f(url(`${lib(libraryId)}/tracks/${encodeURIComponent(trackId)}/waveform`));
	if (res.status === 204 || !res.ok) return null;
	return res.json();
}

export const audioUrl = (libraryId: string, trackId: string, variant: string) =>
	url(`${lib(libraryId)}/tracks/${encodeURIComponent(trackId)}/audio`, { variant });

/** An image at `size` CSS pixels, fetched at the device's pixel density. */
export function imageUrl(libraryId: string, image: ImageRef, size: number): string {
	const dpr = typeof devicePixelRatio === 'number' ? Math.min(devicePixelRatio, 3) : 1;
	return url(`${lib(libraryId)}/images/${encodeURIComponent(image.id)}`, {
		size: Math.round(size * dpr)
	});
}

/** Every page of a cursor-paged list. For bounded lists only: an album, a playlist. */
export async function all<T>(
	load: (cursor: string | undefined) => Promise<{ items: T[]; nextCursor: string | null }>
): Promise<T[]> {
	const items: T[] = [];
	let cursor: string | undefined;
	do {
		const page = await load(cursor);
		items.push(...page.items);
		cursor = page.nextCursor ?? undefined;
	} while (cursor);
	return items;
}
