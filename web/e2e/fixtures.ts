// A tiny library served by intercepting /api/v1, so the tests need no music folder.

import { mkdtempSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Page, Route } from '@playwright/test';
import type { PluginManifest } from '../src/lib/api/plugins';
import type {
	AlbumSummary,
	Library,
	PlayReport,
	PlayerState,
	QueueEdit,
	QueueQuery,
	RealtimeClientMessage,
	RealtimeServerMessage,
	RecentSearch,
	Session,
	SetupRequest,
	TrackSummary
} from '../src/lib/api/types';
import { initSync, Queue } from '../src/lib/core-wasm/jewelcase_core_wasm.js';
import { type SortField, sortTracks } from '../src/lib/sorting';
import { withManifest } from '../../tools/plugin-pack/manifest.mjs';
import {
	type Generated,
	type PlaylistLibrary,
	PlaylistStore,
	playlistRoute
} from '../mock/playlists';
import { PluginStore, adminRoute } from '../mock/plugins';

initSync({
	module: readFileSync(new URL('../src/lib/core-wasm/jewelcase_core_wasm_bg.wasm', import.meta.url))
});

export const library: Library = {
	id: '11',
	name: 'Test Library',
	trackCount: 3,
	albumCount: 1,
	artistCount: 1,
	durationUs: 6_000_000,
	scanning: false
};

export const artist = { id: '21', name: 'Aurora Lane' };

export const album: AlbumSummary = {
	id: '31',
	title: 'Signal',
	artists: [artist],
	releaseDate: '2023',
	type: 'album',
	trackCount: 3,
	trackTotal: 3,
	discCount: 2,
	discTotal: 2,
	durationUs: 6_000_000,
	image: null,
	availability: 'available'
};

const track = (
	id: string,
	title: string,
	disc: number,
	n: number,
	addedAt: string
): TrackSummary => ({
	id,
	title,
	addedAt,
	artists: [artist],
	album: { id: album.id, title: album.title, artists: [artist], image: null },
	discNumber: disc,
	trackNumber: n,
	durationUs: 2_000_000,
	explicit: false,
	audio: { codec: 'pcm', lossless: true },
	availability: 'available',
	sortKeys: { title: title.toLowerCase(), artist: 'aurora lane', album: 'signal' }
});

// Added out of title order, so newest first is an order of its own.
export const tracks = [
	track('41', 'Signal Part 1', 1, 1, '2026-01-01T00:00:00.000Z'),
	track('42', 'Signal Part 2', 1, 2, '2026-01-03T00:00:00.000Z'),
	track('43', 'Signal Remix', 2, 1, '2026-01-02T00:00:00.000Z')
];

/** The library's tracks as `listTracks` would sort them: `sort` and `order` from `query`. */
function sortedAsAsked(query: URLSearchParams): TrackSummary[] {
	const desc = query.get('order') === 'desc';
	const sort = query.get('sort') ?? 'name';
	if (sort === 'dateAdded') {
		const oldest = tracks.toSorted((a, b) => a.addedAt!.localeCompare(b.addedAt!));
		return desc ? oldest.toReversed() : oldest;
	}
	// The server's own order is tested on the server; this stands in for it with the client's.
	const fields: Record<string, SortField> = {
		name: 'title',
		artist: 'artist',
		album: 'album',
		releaseDate: 'year',
		duration: 'duration'
	};
	return [...sortTracks(tracks, { field: fields[sort] ?? 'album', order: desc ? 'desc' : 'asc' })];
}

/** The tracks a queue's list `query` reads to its end, as the server runs it. */
function resolveQuery(query: QueueQuery): TrackSummary[] {
	if (query.type === 'search') {
		return tracks.filter((t) => t.title.toLowerCase().includes(query.q.toLowerCase()));
	}
	const found =
		(query.albumId ?? album.id) === album.id && (query.artistId ?? artist.id) === artist.id;
	const asked = new URLSearchParams({ sort: query.sort ?? 'name', order: query.order ?? 'asc' });
	return found ? sortedAsAsked(asked) : [];
}

/**
 * The account's queue as the server keeps it: the shared core in Node applies every start and
 * edit, and what it applied is sent to the page over the realtime connection `serveRealtime`
 * answers, if one is, as the server does. One per page.
 */
class FixtureQueue {
	queue: Queue | null = null;
	queueId = '';
	version = 0;
	context: object = {};
	/** The editIds answered, with the version each made. */
	#answered = new Map<string, number>();
	/** Sends a message to the page's realtime connection, when there is one. */
	hear: (message: RealtimeServerMessage) => void = () => {};

	/** The queue as `GET /me/queue` answers it. */
	snapshot() {
		return {
			queueId: this.queueId,
			version: this.version,
			context: this.context,
			state: JSON.parse(this.queue!.snapshot())
		};
	}

	/** Starts a queue as `POST /me/queue` does, carrying the Manual Queue over. */
	start(body: {
		queueId: string;
		context: object;
		source: { key: string; libraryId: string; tracks?: string[]; query?: QueueQuery };
		start?: number;
		startTrackId?: string;
		shuffle?: { seed: number };
	}) {
		if (body.queueId === this.queueId) return;
		const { query, ...source } = body.source;
		const list = query ? resolveQuery(query).map((t) => t.id) : (source.tracks ?? []);
		const first = query ? list.indexOf(body.startTrackId!) : body.start!;
		const next = Queue.start(JSON.stringify({ ...source, tracks: list }), first);
		if (this.queue) next.carryManual(this.queue);
		if (body.shuffle) {
			next.apply(JSON.stringify({ type: 'shuffle', on: true, seed: body.shuffle.seed }));
		}
		this.queue = next;
		this.queueId = body.queueId;
		this.context = body.context;
		this.version = 0;
		this.#answered.clear();
		this.hear({ type: 'queueReplaced', queueId: this.queueId });
	}

	/** Applies an edit as `POST /me/queue/edits` does: the version it made, or the problem code. */
	edit(body: {
		editId: string;
		queueId: string;
		edit: QueueEdit;
	}): number | 'queue_changed' | 'anchor_gone' {
		if (!this.queue || body.queueId !== this.queueId) return 'queue_changed';
		const done = this.#answered.get(body.editId);
		if (done !== undefined) return done;
		try {
			this.queue.apply(JSON.stringify(body.edit));
		} catch {
			return 'anchor_gone';
		}
		this.#answered.set(body.editId, ++this.version);
		this.hear({
			type: 'queueEdit',
			queueId: this.queueId,
			version: this.version,
			editId: body.editId,
			edit: body.edit
		});
		return this.version;
	}
}

const queues = new WeakMap<Page, FixtureQueue>();
/** The queue the page's fixtures share, made on first use. */
const queueOf = (page: Page) => {
	if (!queues.has(page)) queues.set(page, new FixtureQueue());
	return queues.get(page)!;
};

/** A playlist of the whole album, for tests that need one to start with. */
export const signalMix: Generated = {
	id: 'pl-signal',
	title: 'Signal Mix',
	description: 'Everything from Signal.',
	createdAt: '2026-01-01T00:00:00.000Z',
	trackIds: tracks.map((t) => BigInt(t.id))
};

/** An empty playlist, to add to. */
export const roadTrip: Generated = {
	id: 'pl-road',
	title: 'Road Trip',
	description: '',
	createdAt: '2026-01-02T00:00:00.000Z',
	trackIds: []
};

/** A playlist long enough to scroll: the album, over and over. */
export const longMix: Generated = {
	...signalMix,
	id: 'pl-long',
	title: 'Long Mix',
	trackIds: Array.from({ length: 15 }, () => signalMix.trackIds).flat()
};

/** The library above, for the mock's playlist store, starting with `playlists`. */
const playlistLibrary = (playlists: Generated[]): PlaylistLibrary => ({
	generated: () => playlists,
	expand(_, ref) {
		const found =
			ref.type === 'track'
				? tracks.filter((t) => t.id === ref.id)
				: ref.id === (ref.type === 'album' ? album.id : artist.id)
					? tracks
					: [];
		return found.length ? found.map((t) => BigInt(t.id)) : undefined;
	},
	track: (_, id) => tracks.find((t) => t.id === String(id))
});

/** Two seconds of 8 kHz mono silence. */
function wav(): Buffer {
	const samples = 16_000;
	const b = Buffer.alloc(44 + samples);
	b.write('RIFF', 0);
	b.writeUInt32LE(36 + samples, 4);
	b.write('WAVEfmt ', 8);
	b.writeUInt32LE(16, 16);
	b.writeUInt16LE(1, 20);
	b.writeUInt16LE(1, 22);
	b.writeUInt32LE(8000, 24);
	b.writeUInt32LE(8000, 28);
	b.writeUInt16LE(1, 32);
	b.writeUInt16LE(8, 34);
	b.write('data', 36);
	b.writeUInt32LE(samples, 40);
	b.fill(128, 44);
	return b;
}

const device = { name: 'Chrome on Linux', type: 'desktop' as const, platform: 'web' };
/** The ID the server gives this browser's device. */
const deviceId = '2';

/** The session a successful setup or login returns to the web client, which gets no token. */
const session = (request: SetupRequest): Session => ({
	user: {
		id: '1',
		username: request.username,
		displayName: request.displayName ?? request.username,
		role: 'owner',
		hasAvatar: false
	},
	device: {
		id: deviceId,
		...request.device,
		firstSeenAt: '2026-09-29T00:00:00.000Z',
		lastSeenAt: '2026-09-29T00:00:00.000Z',
		connected: false
	}
});

const pageOf = <T>(items: T[]) => ({ items, total: items.length, nextCursor: null });

export const pluginManifest: PluginManifest = {
	id: 'lrclib-lyrics',
	name: 'LRCLIB Lyrics',
	version: '0.1.0',
	apiVersion: '0.1',
	description: 'Fetches synced lyrics for tracks that have none.',
	settings: {
		properties: {
			syncedOnly: { type: 'boolean', title: 'Synced lyrics only', default: false },
			apiKey: { type: 'string', title: 'API key', writeOnly: true }
		}
	},
	permissions: [
		{ permission: 'libraryRead', required: true, reason: 'To find tracks without lyrics.' },
		{
			permission: 'network',
			required: true,
			reason: 'To fetch lyrics.',
			destinations: ['lrclib.net']
		},
		{ permission: 'libraryAdd', required: false, reason: 'To save .lrc files beside tracks.' }
	]
};

/** A packed plugin: an empty component carrying the manifest. */
export function pluginFile() {
	const component = new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00]);
	return {
		name: 'lrclib-lyrics.wasm',
		mimeType: 'application/wasm',
		buffer: Buffer.from(withManifest(component, pluginManifest))
	};
}

/** Lyrics for the first track, as the scanner stores them from a `.lrc`. */
export const syncedLyrics = {
	kind: 'synced',
	plain: null,
	lines: [
		{ startMs: 0, text: 'First line of the song' },
		{ startMs: 1000, text: 'Second line of the song' }
	]
};

/** Stands in for a plugin run, which needs the server, so tests never touch a real library. */
async function fakeRun(id: string) {
	return {
		ok: true,
		summary: `Saved lyrics for 2 of 3 tracks (2 synced, 0 plain); 1 not found.`,
		log: [
			`✓ Signal Part 1 — Aurora Lane: saved synced lyrics (${id})`,
			'· Signal Remix — Aurora Lane: not found'
		],
		saved: 2,
		scannerRunning: true
	};
}

/** The password `POST /auth/login` accepts, for any username. */
export const PASSWORD = 'correct horse battery staple';

/**
 * A server whose setup is done unless `setupRequired`, when `POST /setup` completes it, and
 * with this browser logged in, as an account with `role`, unless `signedIn` is false. Until
 * then, anything but setup and login answers `401`. Returns every play reported to it, and its
 * recent searches, newest first.
 */
export async function serveLibrary(
	page: Page,
	{
		empty = false,
		setupRequired = false,
		signedIn = true,
		role = 'owner' as 'owner' | 'admin' | 'user',
		playlists = [] as Generated[]
	} = {}
) {
	// Plugin administration runs through the mock's own logic, on a fresh directory per test.
	const plugins = new PluginStore(mkdtempSync(join(tmpdir(), 'jc-e2e-plugins-')), () =>
		empty ? [] : [library.id]
	);
	// Playlists, too, run through the mock's own store.
	const playlistStore = new PlaylistStore(playlistLibrary(playlists));
	const plays: PlayReport[] = [];
	const queue = queueOf(page);
	const searches: RecentSearch[] = [];
	// A scrobbler this user can connect with their own token, which it never shows again, and
	// share their searches with once connected.
	let token: string | null = null;
	let sharesSearches = false;
	const mySettings = () => ({
		schema: {
			type: 'object',
			properties: { token: { type: 'string', title: 'User token', writeOnly: true } },
			required: ['token']
		},
		values: {},
		secretsSet: token ? ['token'] : []
	});
	// On the context, so the queue's worker is answered too: a page's routes do not reach it.
	await page.context().route('**/api/v1/**', async (route: Route) => {
		const url = new URL(route.request().url());
		const path = url.pathname.replace(/^.*\/api\/v1/, '');
		const lib = `/libraries/${library.id}`;
		const json = (body: unknown) => route.fulfill({ json: body });
		const reply = (out: { status: number; body?: unknown } | undefined) =>
			!out
				? route.fulfill({ status: 404 })
				: out.body === undefined
					? route.fulfill({ status: out.status })
					: route.fulfill({ status: out.status, json: out.body });
		const problem = (status: number, code: string, detail?: string) =>
			route.fulfill({ status, json: { type: 'about:blank', title: '', status, code, detail } });

		if (path === '/server') return json({ version: 'test', apiVersion: '1', setupRequired });
		if (path === '/setup' && setupRequired) {
			const request = route.request().postDataJSON();
			if (request.password.length < 12) {
				return problem(422, 'weak_password', 'This is a very common password.');
			}
			setupRequired = false;
			signedIn = true;
			return route.fulfill({ status: 201, json: session(request) });
		}
		if (setupRequired) return problem(503, 'setup_required');
		if (path === '/auth/login') {
			const request = route.request().postDataJSON();
			if (request.password !== PASSWORD) return problem(401, 'invalid_credentials');
			signedIn = true;
			return json(session(request));
		}
		if (!signedIn) return problem(401, 'unauthenticated');
		if (path === '/auth/logout') {
			signedIn = false;
			return route.fulfill({ status: 204 });
		}
		if (path === '/me/plays') {
			plays.push(...route.request().postDataJSON().items);
			return route.fulfill({ status: 204 });
		}
		if (path === '/me/queue') {
			if (route.request().method() === 'GET') {
				return queue.queue ? json(queue.snapshot()) : problem(404, 'not_found');
			}
			queue.start(route.request().postDataJSON());
			return route.fulfill({ status: 204 });
		}
		if (path === '/me/queue/edits') {
			const version = queue.edit(route.request().postDataJSON());
			return typeof version === 'string' ? problem(409, version) : json({ version });
		}
		if (path === '/me/plugins') {
			return json({
				items: [
					{
						id: 'scrobbler',
						name: 'Scrobbler',
						connected: token !== null,
						asksForSearches: true,
						sharesSearches
					}
				]
			});
		}
		if (path === '/me/plugins/scrobbler/search-sharing') {
			const { sharing } = route.request().postDataJSON();
			if (sharing && token === null) {
				return problem(
					422,
					'validation_failed',
					'Connect it before sharing your searches with it.'
				);
			}
			sharesSearches = sharing;
			return route.fulfill({ status: 204 });
		}
		if (path === '/me/plugins/scrobbler/settings') {
			const method = route.request().method();
			if (method === 'DELETE') {
				token = null;
				sharesSearches = false;
				return route.fulfill({ status: 204 });
			}
			if (method === 'PUT') {
				const entered = route.request().postDataJSON().values.token ?? token;
				if (entered !== 'good-token') {
					return problem(422, 'plugin_settings_invalid', 'The scrobbler refused this token.');
				}
				token = entered;
			}
			return json(mySettings());
		}
		if (path === '/me') {
			return json({ ...session({ username: 'sam', password: '', device }).user, role });
		}
		// Plugins are administered, so a user is refused (`requirements/users.md` §10).
		if (path.startsWith('/admin/') && role === 'user') return problem(403, 'forbidden');
		if (path.startsWith('/admin/plugins')) {
			const req = route.request();
			const out = await adminRoute(
				plugins,
				req.method(),
				path + url.search,
				req.headers()['content-type'] ?? '',
				new Uint8Array(req.postDataBuffer() ?? []),
				fakeRun
			);
			return reply(out);
		}

		if (path === '/libraries') return json({ items: empty ? [] : [library] });
		if (path === `${lib}/albums`) return json(pageOf([album]));
		if (path === `${lib}/albums/${album.id}`) {
			return json({
				...album,
				labels: [],
				discs: [
					{ number: 1, trackCount: 2, trackTotal: 2 },
					{ number: 2, trackCount: 1, trackTotal: 1 }
				]
			});
		}
		if (path === `${lib}/tracks` && url.searchParams.has('ids')) {
			const found = url.searchParams
				.get('ids')!
				.split(',')
				.map((id) => tracks.find((t) => t.id === id));
			return json(pageOf(found.filter((t) => t !== undefined)));
		}
		if (path === `${lib}/tracks`) return json(pageOf(sortedAsAsked(url.searchParams)));
		if (path === `${lib}/artists`) {
			return json(pageOf([{ ...artist, image: null, albumCount: 1, trackCount: 3 }]));
		}
		if (path === `${lib}/artists/${artist.id}`) {
			return json({
				...artist,
				image: null,
				albumCount: 1,
				trackCount: 3,
				addedAt: '2026-01-01T00:00:00.000Z',
				biography: null,
				genres: [],
				appearanceCount: 0
			});
		}
		if (path.startsWith(`${lib}/playlists`)) {
			const req = route.request();
			return reply(
				playlistRoute(
					playlistStore,
					BigInt(library.id),
					req.method(),
					path.slice(lib.length) + url.search,
					new Uint8Array(req.postDataBuffer() ?? []),
					req.headers()['idempotency-key']
				)
			);
		}
		const recent = new RegExp(`^${lib}/recent-searches(?:/(\\d+))?$`).exec(path);
		if (recent) {
			const method = route.request().method();
			if (method === 'POST') {
				const { query, selected } = route.request().postDataJSON();
				const at = searches.findIndex((r) => r.query.toLowerCase() === query.toLowerCase());
				if (at >= 0) searches.splice(at, 1);
				const search = {
					id: String(Date.now() + searches.length),
					query,
					selected: selected ?? null,
					searchedAt: new Date().toISOString()
				};
				searches.unshift(search);
				return route.fulfill({ status: 201, json: search });
			}
			if (method === 'DELETE') {
				const at = recent[1] ? searches.findIndex((r) => r.id === recent[1]) : 0;
				searches.splice(at, recent[1] ? 1 : searches.length);
				return route.fulfill({ status: 204 });
			}
			return json({ items: searches });
		}
		if (path === `${lib}/search`) {
			const q = url.searchParams.get('q') ?? '';
			const hits = tracks.filter((t) => t.title.toLowerCase().includes(q.toLowerCase()));
			return json({
				query: q,
				top: hits[0] ? { type: 'track', track: hits[0] } : null,
				sections: hits.length
					? [
							{
								type: 'tracks',
								items: hits.map((t) => ({ type: 'track', track: t })),
								total: hits.length
							}
						]
					: [],
				nearMisses: [],
				libraryEmpty: false
			});
		}
		const one = tracks.find((t) => path === `${lib}/tracks/${t.id}`);
		if (one) {
			const lyrics = one.id === tracks[0].id ? 'synced' : 'none';
			return json({ ...one, lyrics, loudness: null, identifiers: {} });
		}
		const media = /^\/libraries\/\d+\/tracks\/(\d+)\/(playback|waveform|audio|lyrics)$/.exec(path);
		if (media?.[2] === 'lyrics') {
			return json(
				media[1] === tracks[0].id ? syncedLyrics : { kind: 'none', lines: null, plain: null }
			);
		}
		if (media?.[2] === 'playback') {
			return json({
				variant: 'original',
				delivery: 'direct',
				codec: 'pcm',
				container: 'wav',
				sampleRateHz: 8000,
				channels: 1,
				transport: 'progressive'
			});
		}
		if (media?.[2] === 'waveform') return route.fulfill({ status: 204 });
		if (media?.[2] === 'audio') return route.fulfill({ body: wav(), contentType: 'audio/wav' });
		return problem(404, 'not_found');
	});
	return { plays, searches };
}

/** Another of the account's devices, connected, which the realtime fixture has playing. */
export const phone = {
	id: '3',
	name: 'Pixel Phone',
	type: 'phone' as const,
	firstSeenAt: '2026-09-29T00:00:00.000Z',
	lastSeenAt: '2026-09-29T00:00:00.000Z',
	connected: true
};

/**
 * Answers the realtime connection as a server would with `phone` playing the second track: it
 * says hello, sends the state, and lists this device and the phone. It carries out `play` and
 * `pause` as if the phone had, and fails any other command as one the phone did not answer. It
 * takes reports as the server does: from the device playing, or one that starts playing.
 * Returns every message the page sent.
 */
export async function serveRealtime(page: Page) {
	const sent: RealtimeClientMessage[] = [];
	const now = () => new Date().toISOString();
	// The phone plays the second track of a queue of the library, which this device then holds.
	const queue = queueOf(page);
	queue.start({
		queueId: crypto.randomUUID(),
		context: { type: 'library' },
		source: { key: crypto.randomUUID(), libraryId: library.id, tracks: tracks.map((t) => t.id) },
		start: 1
	});
	let state: PlayerState = {
		status: 'playing',
		activeDeviceId: phone.id,
		itemId: queue.queue!.current() ?? null,
		libraryId: library.id,
		trackId: tracks[1].id,
		positionMs: 0,
		positionAt: now(),
		volume: 1
	};
	await page.routeWebSocket('**/api/v1/realtime', (ws) => {
		const send = (message: RealtimeServerMessage) => ws.send(JSON.stringify(message));
		queue.hear = send;
		const devices = [
			{ ...phone },
			{
				id: deviceId,
				...device,
				firstSeenAt: phone.firstSeenAt,
				lastSeenAt: phone.lastSeenAt,
				connected: true
			}
		];
		send({ type: 'hello', deviceId, serverTime: now() });
		send({ type: 'player', state });
		send({ type: 'devices', devices });
		ws.onMessage((data) => {
			const message: RealtimeClientMessage = JSON.parse(data.toString());
			sent.push(message);
			if (message.type === 'ping') send({ type: 'pong', serverTime: now() });
			if (message.type === 'playerCommand') {
				const { commandId, type } = message.command;
				send({ type: 'commandResult', result: { commandId, status: 'pending' } });
				if (type === 'play' || type === 'pause') {
					state = { ...state, status: type === 'play' ? 'playing' : 'paused', positionAt: now() };
					send({ type: 'commandResult', result: { commandId, status: 'applied' } });
					send({ type: 'player', state });
				} else {
					const reason = 'The device did not answer.';
					send({ type: 'commandResult', result: { commandId, status: 'failed', reason } });
				}
			}
			const report = message.type === 'progress' ? message.report : null;
			const starts = report?.start && report.status !== 'stopped';
			if (report && (starts || deviceId === state.activeDeviceId)) {
				state = {
					...state,
					status: report.status === 'buffering' ? 'paused' : report.status,
					activeDeviceId: report.status === 'stopped' ? null : deviceId,
					itemId: report.itemId,
					libraryId: report.libraryId,
					trackId: report.trackId,
					positionMs: report.positionMs,
					positionAt: now(),
					volume: report.volume ?? (deviceId === state.activeDeviceId ? state.volume : null)
				};
				send({ type: 'player', state });
			}
		});
	});
	return { sent };
}
