// Typed calls to the public API (`api/openapi.yaml`). Pages load through here. Catalog reads
// go through `catalog/read.ts`, which answers from the device or the server
// (`design/general.md` §7.1).

import { redirect } from '@sveltejs/kit';
import { connectivity } from '$lib/connectivity.svelte';
import { gated } from '$lib/gated-fetch';
import { catalog } from '$lib/catalog/client.svelte';
import { type LastKnown, lastKnown, owner, remember } from '$lib/last-known';
import { read, readSearch, unavailableIfNotOnline, unreachable } from '$lib/catalog/read';
import { API, ApiError, loginUrl } from './root';
import type {
	Album,
	AlbumPage,
	AppliedEdit,
	Artist,
	ArtistPage,
	EditQueue,
	ImageRef,
	KeptQueue,
	Library,
	LoginRequest,
	Lyrics,
	PlaybackInfo,
	PlayReport,
	Playable,
	Playlist,
	PlaylistCreate,
	PlaylistEdit,
	PlaylistItemPage,
	PlaylistPage,
	RecentSearch,
	RestoredQueue,
	Scan,
	SearchResponse,
	ServerInfo,
	Session,
	SetupRequest,
	StartQueue,
	StartedQueue,
	Tag,
	TagPage,
	TagType,
	Track,
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

export { API, ApiError, loginUrl };

function url(path: string, query: Query = {}): string {
	const params = new URLSearchParams();
	for (const [k, v] of Object.entries(query)) if (v !== undefined) params.set(k, String(v));
	const qs = params.toString();
	return `${API}${path}${qs ? `?${qs}` : ''}`;
}

/** Set once a session that was good has ended here and the page is to reload at the login page. */
let ending = false;

/**
 * Whether the page is about to reload at the login page because a session that was good ended,
 * so the login page shown meanwhile leaves what it would tell once for the page after the reload.
 */
export const sessionEnding = () => ending;

/**
 * The response's JSON, or its Problem as an `ApiError` whose message is safe to show. A
 * request the session no longer covers redirects to the login page. If the session had been
 * good, the device is wiped and the page then reloads at the login page, so nothing of the
 * session, playback above all, outlives it. Without one, the redirect takes effect where a page
 * is loading, and is an error like any other elsewhere.
 */
async function body<T>(res: Response): Promise<T> {
	if (res.status === 204) return undefined as T;
	if (!res.ok) {
		const problem = await res.json().catch(() => null);
		if (problem?.code === 'unauthenticated' || problem?.code === 'session_revoked') {
			const login = loginUrl();
			// A session that was good has ended: the device keeps nothing of it.
			if (owner() && !ending) {
				ending = true;
				void catalog
					.signOut()
					.catch(console.error)
					.finally(() => location.assign(login));
			}
			redirect(307, login);
		}
		const message = problem?.detail ?? problem?.title ?? res.statusText;
		const retryAfter = Number(res.headers.get('Retry-After')) || undefined;
		throw new ApiError(res.status, problem?.code, message, retryAfter);
	}
	return res.json();
}

/**
 * `fetch`, or a failure as with no network while the device is offline or working offline. Any
 * answer below 500 returns connectivity to online; a request that fails to get one makes it
 * unreachable.
 */
const request = async (fetch: Fetch, input: string, init?: RequestInit) => {
	try {
		const response = await gated(() => connectivity.blocked, fetch)(input, init);
		// A 5xx, such as a proxy's 502, is not the server answering.
		if (response.status < 500) connectivity.reach();
		return response;
	} catch (error) {
		if (error instanceof TypeError && !connectivity.blocked) connectivity.lose();
		throw error;
	}
};

/**
 * A GET of `path` at the server, rejecting as unavailable when the request cannot be sent and the
 * device is not online.
 */
const get = async <T>(fetch: Fetch, path: string, query?: Query): Promise<T> =>
	body(await request(fetch, url(path, query)).catch(unavailableIfNotOnline));

/**
 * A state-changing request, with a JSON body or a file. Cookie-authenticated writes must
 * carry `X-Jewelcase-Client` or they are refused (`api/openapi.yaml`, Authentication).
 * With `idempotencyKey`, a retry returns the first attempt's result rather than applying twice.
 */
async function send<T>(
	fetch: Fetch,
	method: 'POST' | 'PUT' | 'PATCH' | 'DELETE',
	path: string,
	json?: Blob | object,
	idempotencyKey?: string
): Promise<T> {
	const headers: Record<string, string> = { 'X-Jewelcase-Client': 'web' };
	if (idempotencyKey) headers['Idempotency-Key'] = idempotencyKey;
	let payload: BodyInit | undefined;
	if (json instanceof Blob) {
		headers['Content-Type'] = 'application/octet-stream';
		payload = json;
	} else if (json !== undefined) {
		headers['Content-Type'] = 'application/json';
		payload = JSON.stringify(json);
	}
	return body(await request(fetch, url(path), { method, headers, body: payload }));
}

/** How long the layout waits for the server before starting with what was kept. */
const KEPT_WAIT_MS = 3000;

/**
 * The server did not answer a kept read in time and has not answered since, so the start does not
 * wait for it again.
 */
let hanging = false;

/**
 * `answer`, kept for the next start with no network; with the server unreachable, or not
 * answering within `KEPT_WAIT_MS` (once, not for each read), what was kept of `key` instead, if
 * anything. Only the layout's own reads use it, so the app can start offline.
 */
const kept = <K extends keyof LastKnown>(key: K, answer: Promise<NonNullable<LastKnown[K]>>) => {
	const known = lastKnown()[key];
	const settled = answer.then(
		(value) => ((hanging = false), remember({ [key]: value }), value),
		(error) => {
			if (known && unreachable(error)) return known;
			throw error;
		}
	);
	if (!known) return settled;
	if (hanging) {
		settled.catch(() => {});
		return Promise.resolve(known);
	}
	let timer: ReturnType<typeof setTimeout>;
	const slow = new Promise<typeof known>(
		(resolve) => (timer = setTimeout(() => ((hanging = true), resolve(known)), KEPT_WAIT_MS))
	);
	return Promise.race([settled, slow]).finally(() => clearTimeout(timer));
};

/** The server's public details; kept for starting offline. */
export const getServerInfo = (f: Fetch) => kept('server', get<ServerInfo>(f, '/server'));

/** Creates the owner and logs this browser in: the server sets the session cookie. */
export const completeSetup = (f: Fetch, request: SetupRequest) =>
	send<Session>(f, 'POST', '/setup', request);

/** Logs this browser in as a new device: the server sets the session cookie. */
export const login = (f: Fetch, request: LoginRequest) =>
	send<Session>(f, 'POST', '/auth/login', request);

/** Logs this browser out: the server forgets the device and clears the cookie. */
export const logout = (f: Fetch) => send<void>(f, 'POST', '/auth/logout');

/** One page of the account's kept queues, most recently active first; `cursor` is the last page's `nextCursor`. */
export const listKeptQueues = (f: Fetch, cursor?: string) =>
	get<{ items: KeptQueue[]; nextCursor: string | null }>(f, '/me/queues', { cursor });

/** Deletes a kept queue; the active queue is refused with `queue_active`. */
export const deleteKeptQueue = (f: Fetch, queueId: string, idempotencyKey?: string) =>
	send<void>(f, 'DELETE', `/me/queues/${encodeURIComponent(queueId)}`, undefined, idempotencyKey);

/** Starts a queue as the account's active one, parking the one it replaces; answers its list. */
export const startQueue = (f: Fetch, start: StartQueue, idempotencyKey?: string) =>
	send<StartedQueue>(f, 'POST', '/me/queue', start, idempotencyKey);

/** Applies an edit to the active queue `queueId`; answers the version it made. */
export const editQueue = (f: Fetch, edit: EditQueue, idempotencyKey?: string) =>
	send<AppliedEdit>(f, 'POST', '/me/queue/edits', edit, idempotencyKey);

/**
 * Makes the kept queue `queueId` the active one; answers it, and the run its `ETag` names
 * (`"<run>.<queueId>.<version>"`), which a later fetch of the queue asks with.
 */
export async function restoreKeptQueue(
	f: Fetch,
	queueId: string,
	idempotencyKey?: string
): Promise<{ queue: RestoredQueue; run: string | null }> {
	const headers: Record<string, string> = { 'X-Jewelcase-Client': 'web' };
	if (idempotencyKey) headers['Idempotency-Key'] = idempotencyKey;
	const path = `/me/queues/${encodeURIComponent(queueId)}/restore`;
	const response = await request(f, url(path), { method: 'POST', headers });
	const queue = await body<RestoredQueue>(response);
	const run = response.headers.get('ETag')?.replace(/^"/, '').split('.')[0] || null;
	return { queue, run };
}

/** The signed-in user; kept for starting offline. */
export const getMe = (f: Fetch) => kept('me', get<User>(f, '/me'));

/** Records plays. With `keepalive`, the request outlives the page, for one closing. */
export const recordPlays = (f: Fetch, items: PlayReport[], keepalive = false) =>
	send<void>((input, init) => f(input, { ...init, keepalive }), 'POST', '/me/plays', { items });

const lib = (libraryId: string) => `/libraries/${encodeURIComponent(libraryId)}`;

/**
 * A catalog read the device can answer, as the operation `name` of `libraryId` with the API's
 * `params`, and the server's answer to the same request at `path` (`catalog/read.ts` says which
 * answers). `path` params, such as an ID, go in `ids` by their names in the API. With `fresh`,
 * the server alone answers: for a read that a write is based on, which the device may hold
 * stale until its catch-up finishes.
 */
const device = <T>(
	f: Fetch,
	name: string,
	libraryId: string,
	path: string,
	query: Query = {},
	ids: Query = {},
	fresh = false
) => {
	const server = () => get<T>(f, `${lib(libraryId)}${path}`, query);
	return fresh ? server() : read<T>(name, libraryId, { ...ids, ...query }, server);
};

/** The libraries the user can reach; kept for starting offline. */
export const listLibraries = (f: Fetch) =>
	kept('libraries', get<{ items: Library[] }>(f, '/libraries'));

/** A page of the library's albums. */
export const listAlbums = (f: Fetch, libraryId: string, query: Query = {}) =>
	device<AlbumPage>(f, 'listAlbums', libraryId, '/albums', query);

/** One album of the library. */
export const getAlbum = (f: Fetch, libraryId: string, albumId: string) =>
	device<Album>(
		f,
		'getAlbum',
		libraryId,
		`/albums/${encodeURIComponent(albumId)}`,
		{},
		{ albumId }
	);

/** A page of the library's tracks. */
export const listTracks = (f: Fetch, libraryId: string, query: Query = {}) =>
	device<TrackPage>(f, 'listTracks', libraryId, '/tracks', query);

/**
 * The tracks `ids` names that `libraryId` holds, in the order named; an ID the library does not
 * hold is left out. At most 100, and never none: with no `ids` the endpoint lists the library.
 * A device that answers fewer than asked may not have synced the rest yet, so while online the
 * server answers the batch instead. With `fresh`, the server alone answers: for the server's
 * word on a track, which the device may hold stale until its catch-up finishes.
 */
export const getTracksByIds = async (f: Fetch, libraryId: string, ids: string[], fresh = false) => {
	let fromServer = false;
	const server = () => {
		fromServer = true;
		return get<TrackPage>(f, `${lib(libraryId)}/tracks`, { ids: ids.join(',') });
	};
	if (fresh) return server();
	const page = await read<TrackPage>('listTracksByIds', libraryId, { ids: ids.join(',') }, server);
	if (fromServer || page.items.length >= new Set(ids).size || connectivity.blocked) return page;
	return server().catch((error) => {
		if (connectivity.state !== 'online' && unreachable(error)) return page;
		throw error;
	});
};

/** One track of the library. */
export const getTrack = (f: Fetch, libraryId: string, trackId: string) =>
	device<Track>(
		f,
		'getTrack',
		libraryId,
		`/tracks/${encodeURIComponent(trackId)}`,
		{},
		{ trackId }
	);

const track = (libraryId: string, trackId: string) =>
	`${lib(libraryId)}/tracks/${encodeURIComponent(trackId)}`;

/**
 * Puts the track at the top of the caller's Liked Songs, and unhides it (`requirements/playlists.md`
 * §11). `likedId` is the ID Liked Songs is made under if the like has to make it again, as the
 * device made it. `key` is the request's idempotency key.
 */
export const likeTrack = (
	f: Fetch,
	libraryId: string,
	trackId: string,
	likedId?: string,
	key?: string
) =>
	send<void>(
		f,
		'PUT',
		`${track(libraryId, trackId)}/like${likedId ? `?likedId=${encodeURIComponent(likedId)}` : ''}`,
		undefined,
		key
	);

/** Takes the track out of the caller's Liked Songs; one not liked is already so. */
export const unlikeTrack = (f: Fetch, libraryId: string, trackId: string, key?: string) =>
	send<void>(f, 'DELETE', `${track(libraryId, trackId)}/like`, undefined, key);

/** Hides the track from the caller's library, and unlikes it (`requirements/conventions.md` §8). */
export const hideTrack = (f: Fetch, libraryId: string, trackId: string, key?: string) =>
	send<void>(f, 'PUT', `${track(libraryId, trackId)}/hidden`, undefined, key);

/** Brings the track back into the caller's library; one not hidden is already so. */
export const unhideTrack = (f: Fetch, libraryId: string, trackId: string, key?: string) =>
	send<void>(f, 'DELETE', `${track(libraryId, trackId)}/hidden`, undefined, key);

/**
 * Hidden Songs: the tracks the caller hid, newest first, a page at a time. With `fresh`, from the
 * server alone.
 */
export const listHiddenTracks = (f: Fetch, libraryId: string, query: Query = {}, fresh = false) =>
	device<TrackPage>(f, 'listHiddenTracks', libraryId, '/hidden-tracks', query, {}, fresh);

/** A page of the library's artists. */
export const listArtists = (f: Fetch, libraryId: string, query: Query = {}) =>
	device<ArtistPage>(f, 'listArtists', libraryId, '/artists', query);

/** The tag types the library's music carries, each with its count of tags (`requirements/tags.md` §2). */
export const listTagTypes = (f: Fetch, libraryId: string) =>
	device<TagType[]>(f, 'listTagTypes', libraryId, '/tag-types');

/** A page of the library's tags. */
export const listTags = (f: Fetch, libraryId: string, query: Query = {}) =>
	device<TagPage>(f, 'listTags', libraryId, '/tags', query);

/** One tag of the library. */
export const getTag = (f: Fetch, libraryId: string, tagId: string) =>
	device<Tag>(f, 'getTag', libraryId, `/tags/${encodeURIComponent(tagId)}`, {}, { tagId });

/** One artist of the library. */
export const getArtist = (f: Fetch, libraryId: string, artistId: string) =>
	device<Artist>(
		f,
		'getArtist',
		libraryId,
		`/artists/${encodeURIComponent(artistId)}`,
		{},
		{ artistId }
	);

/** A page of the library's playlists. */
export const listPlaylists = (f: Fetch, libraryId: string, query: Query = {}) =>
	device<PlaylistPage>(f, 'listPlaylists', libraryId, '/playlists', query);

const playlist = (libraryId: string, playlistId: string) =>
	`${lib(libraryId)}/playlists/${encodeURIComponent(playlistId)}`;

/**
 * One playlist. With `fresh`, from the server alone: use it for the version an edit is based on.
 */
export const getPlaylist = (f: Fetch, libraryId: string, playlistId: string, fresh = false) =>
	device<Playlist>(
		f,
		'getPlaylist',
		libraryId,
		`/playlists/${encodeURIComponent(playlistId)}`,
		{},
		{ playlistId },
		fresh
	);

/** At version 1. To retry, pass the first attempt's `key`, so only one playlist is created. */
export const createPlaylist = (
	f: Fetch,
	libraryId: string,
	request: PlaylistCreate,
	key: string = crypto.randomUUID()
) => send<Playlist>(f, 'POST', `${lib(libraryId)}/playlists`, request, key);

/**
 * Applies all of `edit` against `edit.baseVersion`, or none of it: an `ApiError` with
 * `version_conflict` means the playlist changed elsewhere. To retry, pass the first attempt's
 * `key`, so an edit that did apply is not refused as a conflict with itself.
 */
export const editPlaylist = (
	f: Fetch,
	libraryId: string,
	playlistId: string,
	edit: PlaylistEdit,
	key: string = crypto.randomUUID()
) => send<Playlist>(f, 'PATCH', playlist(libraryId, playlistId), edit, key);

/** Deletes the playlist. To retry, pass the first attempt's `key`. */
export const deletePlaylist = (f: Fetch, libraryId: string, playlistId: string, key?: string) =>
	send<void>(f, 'DELETE', playlist(libraryId, playlistId), undefined, key);

/** Which of the tracks `items` stand for are already in the playlist, to warn before adding. */
export const checkPlaylistDuplicates = (
	f: Fetch,
	libraryId: string,
	playlistId: string,
	items: Playable[]
) =>
	send<{ trackIds: string[] }>(f, 'POST', `${playlist(libraryId, playlistId)}/duplicates`, {
		items
	});

/**
 * A page of a playlist's items. With `fresh`, from the server alone: use it for the items an edit
 * is based on.
 */
export const listPlaylistItems = (
	f: Fetch,
	libraryId: string,
	playlistId: string,
	query: Query = {},
	fresh = false
) =>
	device<PlaylistItemPage>(
		f,
		'listPlaylistItems',
		libraryId,
		`/playlists/${encodeURIComponent(playlistId)}/items`,
		query,
		{ playlistId },
		fresh
	);

/** Search results: from the server when it can be reached, from the device's index otherwise. */
export const search = (f: Fetch, libraryId: string, q: string, query: Query = {}) =>
	readSearch<SearchResponse>('search', libraryId, { q, ...query }, () =>
		get<SearchResponse>(f, `${lib(libraryId)}/search`, { q, ...query })
	);

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
	const res = await request(
		f,
		url(`${lib(libraryId)}/tracks/${encodeURIComponent(trackId)}/waveform`)
	);
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

// ───────────────────────────── Scan administration ─────────────────────────────

const scans = (libraryId: string) => `/admin/libraries/${encodeURIComponent(libraryId)}/scans`;

/** The library's queued and running scans, and the last to finish, newest first. */
export const adminListScans = (f: Fetch, libraryId: string) =>
	get<{ items: Scan[] }>(f, scans(libraryId));

/** Queues a scan of the whole library; answers the scan that will do it, which may be one already queued. */
export const startScan = (f: Fetch, libraryId: string) => send<Scan>(f, 'POST', scans(libraryId));

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

/** Exempts it from, or holds it to, the limit for every plugin at `host`. */
export const setPluginExempt = (f: Fetch, id: string, host: string, exempt: boolean) =>
	send<void>(f, exempt ? 'PUT' : 'DELETE', `${plugin(id)}/exemptions/${encodeURIComponent(host)}`);

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
