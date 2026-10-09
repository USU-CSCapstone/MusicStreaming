// Typed calls to the public API (`api/openapi.yaml`). Pages load through here,
// so this is the one module that changes when catalog reads move to the worker
// (`design/general.md` §7.1).

import { redirect } from '@sveltejs/kit';
import { base, resolve } from '$app/paths';
import type {
	Album,
	AlbumPage,
	Artist,
	ArtistPage,
	ImageRef,
	Library,
	LoginRequest,
	Lyrics,
	PlaybackInfo,
	PlayReport,
	Playlist,
	PlaylistItemPage,
	PlaylistPage,
	RecentSearch,
	SearchResponse,
	ServerInfo,
	Session,
	SetupRequest,
	TrackPage,
	User,
	Waveform
} from './types';
import type {
	PermissionGrants,
	PersonalPlugin,
	Plugin,
	PluginRunResult,
	PluginSettings
} from './plugins';

type Fetch = typeof fetch;
type Query = Record<string, string | number | undefined>;

export const API = `${base}/api/v1`;

export class ApiError extends Error {
	constructor(
		readonly status: number,
		readonly code: string | undefined,
		message: string,
		/** Seconds until trying again is worth it, with `rate_limited`. */
		readonly retryAfter?: number
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

/** The login page, returning to where the browser is now once logged in. */
export const loginUrl = () =>
	`${resolve('/login')}?${new URLSearchParams({ next: location.pathname + location.search })}`;

/**
 * The response's JSON, or its Problem as an `ApiError` whose message is safe to show. A
 * request the session no longer covers redirects to the login page; that takes effect where
 * a page is loading, and is an error like any other elsewhere.
 */
async function body<T>(res: Response): Promise<T> {
	if (res.status === 204) return undefined as T;
	if (!res.ok) {
		const problem = await res.json().catch(() => null);
		if (problem?.code === 'unauthenticated' || problem?.code === 'session_revoked') {
			redirect(307, loginUrl());
		}
		const message = problem?.detail ?? problem?.title ?? res.statusText;
		const retryAfter = Number(res.headers.get('Retry-After')) || undefined;
		throw new ApiError(res.status, problem?.code, message, retryAfter);
	}
	return res.json();
}

const get = async <T>(fetch: Fetch, path: string, query?: Query): Promise<T> =>
	body(await fetch(url(path, query)));

/**
 * A state-changing request, with a JSON body or a file. Cookie-authenticated writes must
 * carry `X-Jewelcase-Client` or they are refused (`api/openapi.yaml`, Authentication).
 */
async function send<T>(
	fetch: Fetch,
	method: 'POST' | 'PUT' | 'DELETE',
	path: string,
	json?: Blob | object
): Promise<T> {
	const headers: Record<string, string> = { 'X-Jewelcase-Client': 'web' };
	let payload: BodyInit | undefined;
	if (json instanceof Blob) {
		headers['Content-Type'] = 'application/octet-stream';
		payload = json;
	} else if (json !== undefined) {
		headers['Content-Type'] = 'application/json';
		payload = JSON.stringify(json);
	}
	return body(await fetch(url(path), { method, headers, body: payload }));
}

export const getServerInfo = (f: Fetch) => get<ServerInfo>(f, '/server');

/** Creates the owner and logs this browser in: the server sets the session cookie. */
export const completeSetup = (f: Fetch, request: SetupRequest) =>
	send<Session>(f, 'POST', '/setup', request);

/** Logs this browser in as a new device: the server sets the session cookie. */
export const login = (f: Fetch, request: LoginRequest) =>
	send<Session>(f, 'POST', '/auth/login', request);

/** Logs this browser out: the server forgets the device and clears the cookie. */
export const logout = (f: Fetch) => send<void>(f, 'POST', '/auth/logout');

export const getMe = (f: Fetch) => get<User>(f, '/me');

/** Records plays. With `keepalive`, the request outlives the page, for one closing. */
export const recordPlays = (f: Fetch, items: PlayReport[], keepalive = false) =>
	send<void>((input, init) => f(input, { ...init, keepalive }), 'POST', '/me/plays', { items });

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

const recent = (libraryId: string) => `${lib(libraryId)}/recent-searches`;

export const listRecentSearches = (f: Fetch, libraryId: string) =>
	get<{ items: RecentSearch[] }>(f, recent(libraryId));

/** Records a settled search. With `keepalive`, the request outlives the page, for one closing. */
export const recordRecentSearch = (
	f: Fetch,
	libraryId: string,
	query: string,
	selected?: RecentSearch['selected'],
	keepalive = false
) =>
	send<RecentSearch>(
		(input, init) => f(input, { ...init, keepalive }),
		'POST',
		recent(libraryId),
		selected ? { query, selected } : { query }
	);

export const deleteRecentSearch = (f: Fetch, libraryId: string, id: string) =>
	send<void>(f, 'DELETE', `${recent(libraryId)}/${encodeURIComponent(id)}`);

export const clearRecentSearches = (f: Fetch, libraryId: string) =>
	send<void>(f, 'DELETE', recent(libraryId));

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

export const getLyrics = (f: Fetch, libraryId: string, trackId: string) =>
	get<Lyrics>(f, `${lib(libraryId)}/tracks/${encodeURIComponent(trackId)}/lyrics`);

export const audioUrl = (libraryId: string, trackId: string, variant: string) =>
	url(`${lib(libraryId)}/tracks/${encodeURIComponent(trackId)}/audio`, { variant });

/** An image at `size` CSS pixels, fetched at the device's pixel density. */
export function imageUrl(libraryId: string, image: ImageRef, size: number): string {
	const dpr = typeof devicePixelRatio === 'number' ? Math.min(devicePixelRatio, 3) : 1;
	return url(`${lib(libraryId)}/images/${encodeURIComponent(image.id)}`, {
		size: Math.round(size * dpr)
	});
}

// ───────────────────────────── Plugin administration ─────────────────────────────

const plugin = (id: string) => `/admin/plugins/${encodeURIComponent(id)}`;

export const adminListPlugins = (f: Fetch) => get<{ items: Plugin[] }>(f, '/admin/plugins');

/** Installed disabled for every library (`requirements/plugins.md` §5). */
export const installPluginFile = (f: Fetch, file: Blob) =>
	send<Plugin>(f, 'POST', '/admin/plugins', file);

export const installPluginUrl = (f: Fetch, pluginUrl: string) =>
	send<Plugin>(f, 'POST', '/admin/plugins', { url: pluginUrl });

export const setPluginPermissions = (f: Fetch, id: string, grants: PermissionGrants) =>
	send<Plugin>(f, 'PUT', `${plugin(id)}/permissions`, grants);

export const setPluginEnabled = (f: Fetch, id: string, libraryId: string, enabled: boolean) =>
	send<Plugin>(f, 'PUT', `${plugin(id)}/libraries/${encodeURIComponent(libraryId)}`, { enabled });

export const uninstallPlugin = (f: Fetch, id: string) => send<void>(f, 'DELETE', plugin(id));

/** Its settings at one level: a library's own, or with no library, the server-wide ones. */
const settingsPath = (id: string, libraryId?: string) =>
	`${plugin(id)}/settings${libraryId ? `?${new URLSearchParams({ libraryId })}` : ''}`;

export const getPluginSettings = (f: Fetch, id: string, libraryId?: string) =>
	get<PluginSettings>(f, settingsPath(id, libraryId));

/** Saves after the server and the plugin check them; a secret left out keeps its value. */
export const setPluginSettings = (
	f: Fetch,
	id: string,
	values: Record<string, unknown>,
	libraryId?: string
) => send<PluginSettings>(f, 'PUT', settingsPath(id, libraryId), { values });

/** The plugins this user can connect with their own account, such as a scrobbler. */
export const listMyPlugins = (f: Fetch) => get<{ items: PersonalPlugin[] }>(f, '/me/plugins');

const mySettings = (id: string) => `/me/plugins/${encodeURIComponent(id)}/settings`;

export const getMyPluginSettings = (f: Fetch, id: string) => get<PluginSettings>(f, mySettings(id));

/** Connects it once the server and the plugin accept them; a secret left out keeps its value. */
export const setMyPluginSettings = (f: Fetch, id: string, values: Record<string, unknown>) =>
	send<PluginSettings>(f, 'PUT', mySettings(id), { values });

export const disconnectMyPlugin = (f: Fetch, id: string) => send<void>(f, 'DELETE', mySettings(id));

/** Turns sharing the caller's searches with a plugin on or off (`requirements/search.md` §6). */
export const setMySearchSharing = (f: Fetch, id: string, sharing: boolean) =>
	send<void>(f, 'PUT', `/me/plugins/${encodeURIComponent(id)}/search-sharing`, { sharing });

/** Runs it once now, with the permissions approved for it; answers when it finishes. */
export const runPlugin = (f: Fetch, id: string) =>
	send<PluginRunResult>(f, 'POST', `${plugin(id)}/run`);

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
