// A tiny library served by intercepting /api/v1, so the tests need no music folder.

import type { Page, Route } from '@playwright/test';
import type {
	AlbumSummary,
	Library,
	PlayReport,
	PlayerState,
	RealtimeClientMessage,
	RealtimeServerMessage,
	StartQueue,
	Tag,
	TagsByType,
	TrackSummary
} from '../src/lib/api/types';
import '../core-node';
import { Queue } from '../src/lib/core-wasm/jewelcase_core_wasm.js';
import type { SortField } from '../src/lib/sorting';
import { sortTracks } from '../src/lib/track-sort';
import { type TagFilter, filterByTags } from '../src/lib/tag-match';

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

/** The library's tags: two genres and a mood. */
export const ambient: Tag = {
	id: '61',
	type: 'genre',
	name: 'Ambient',
	trackCount: 2,
	albumCount: 1,
	artistCount: 1
};
export const electronic: Tag = {
	id: '62',
	type: 'genre',
	name: 'Electronic',
	trackCount: 1,
	albumCount: 1,
	artistCount: 1
};
export const calm: Tag = {
	id: '63',
	type: 'mood',
	name: 'Calm',
	trackCount: 2,
	albumCount: 1,
	artistCount: 1
};
const TAGS = [ambient, electronic, calm];

/** `tags` as an entity carries them: by type, each type's in name order. */
function byType(tags: Tag[]): TagsByType {
	const out: TagsByType = {};
	for (const tag of tags.toSorted((a, b) => a.name.localeCompare(b.name))) {
		(out[tag.type] ??= []).push({ id: tag.id, name: tag.name });
	}
	return out;
}

/** The filter a list's `tagAll` and `tagAny` ask for. */
function filterOf(query: URLSearchParams): TagFilter {
	const ids = (key: string) => (query.get(key) ?? '').split(',').filter(Boolean);
	return { all: ids('tagAll'), any: ids('tagAny') };
}

/** The items of `items` that `filter` keeps, each tag of the type the library gives it. */
function tagged<T>(items: T[], filter: TagFilter, tagsOf: (item: T) => TagsByType): T[] {
	return filterByTags(items, filter, tagsOf, (id) => TAGS.find((t) => t.id === id)?.type);
}

export const album: AlbumSummary = {
	id: '31',
	title: 'Signal',
	artists: [artist],
	releaseDate: '2023',
	type: 'album',
	tags: byType(TAGS),
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
	addedAt: string,
	tags: Tag[]
): TrackSummary => ({
	id,
	title,
	addedAt,
	artists: [artist],
	album: { id: album.id, title: album.title, artists: [artist], image: null },
	discNumber: disc,
	trackNumber: n,
	durationUs: 2_000_000,
	tags: byType(tags),
	explicit: false,
	audio: { codec: 'pcm', lossless: true },
	availability: 'available',
	sortKeys: { title: title.toLowerCase(), artist: 'aurora lane', album: 'signal' },
	liked: false
});

// Added out of title order, so newest first is an order of its own.
export const tracks = [
	track('41', 'Signal Part 1', 1, 1, '2026-01-01T00:00:00.000Z', [ambient, calm]),
	track('42', 'Signal Part 2', 1, 2, '2026-01-03T00:00:00.000Z', [ambient]),
	track('43', 'Signal Remix', 2, 1, '2026-01-02T00:00:00.000Z', [electronic, calm])
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

/**
 * The account's queue as the server keeps it: the shared core in Node holds the state, and the
 * fixture answers the queue's endpoints for it. One per page.
 */
class FixtureQueue {
	queue: Queue | null = null;
	queueId = '';
	context: object = {};
	/** The lists held, by hash. */
	lists = new Map<string, string[]>();

	/** The queue as `GET /me/queue` answers it. */
	snapshot() {
		return {
			queueId: this.queueId,
			version: 0,
			context: this.context,
			state: JSON.parse(this.queue!.toJson())
		};
	}

	/**
	 * Starts a queue as `POST /me/queue` does: the list is the tracks sent, or the library's tracks
	 * as the recipe sorts them, which is all the tests that send one ask for.
	 */
	start(body: StartQueue) {
		const { key, libraryId, tracks: sent, recipe } = body.source;
		const ids =
			sent ??
			sortedAsAsked(new URLSearchParams({ sort: recipe!.sort!, order: recipe!.order! })).map(
				(t) => t.id
			);
		// As the server: `start` if the list holds the track there, else its first place.
		const at =
			sent || ids[body.start] === body.startTrackId ? body.start : ids.indexOf(body.startTrackId!);
		const list = `list-${this.lists.size + 1}`;
		const repeat = this.queue ? JSON.parse(this.queue.toJson()).repeat : 'off';
		const state = JSON.parse(Queue.started(key, libraryId, ids.length, at, repeat).toJson());
		state.sources[0].list = list;
		this.queue?.free();
		this.queue = Queue.fromJson(JSON.stringify(state));
		this.queueId = body.queueId;
		this.context = body.context;
		this.lists.set(list, ids);
		return { list, length: ids.length };
	}

	/** A page of rows as `GET /me/queue/rows` answers it, from the list of every source. */
	rows(query: URLSearchParams) {
		const copy = this.queue!.copy();
		for (const [hash, ids] of this.lists) {
			copy.attachList(hash, JSON.stringify({ libraryId: library.id, tracks: ids }));
		}
		const [offset, limit] = [Number(query.get('offset')), Number(query.get('limit'))];
		const history = query.get('section') === 'history';
		type Row = { item: string; track: string | null; play?: number };
		const row = ({ item, track, play }: Row) => ({
			item,
			libraryId: library.id,
			trackId: track,
			...(play === undefined ? {} : { play })
		});
		const read: Row[] = JSON.parse(
			history ? copy.historyRows(offset, limit) : copy.upNextRows(offset, limit)
		);
		const current = copy.currentRow();
		const page = {
			queueId: this.queueId,
			version: 0,
			current: current && row(JSON.parse(current)),
			total: history ? copy.historyLen() : copy.upNextLen(),
			manual: history ? 0 : copy.manualLen(),
			shuffled: !!JSON.parse(copy.toJson()).shuffle,
			repeat: JSON.parse(copy.toJson()).repeat,
			rows: read.map(row)
		};
		copy.free();
		return page;
	}
}

const queues = new WeakMap<Page, FixtureQueue>();
/** The queue the page's fixtures share, made on first use. */
export const queueOf = (page: Page) => {
	if (!queues.has(page)) queues.set(page, new FixtureQueue());
	return queues.get(page)!;
};

/** Answers the queue's endpoints for `queue`, which its worker asks. */
async function serveQueue(page: Page, queue: FixtureQueue) {
	await page.route('**/api/v1/me/queue**', async (route: Route) => {
		const request = route.request();
		const url = new URL(request.url());
		const path = url.pathname.replace(/^.*\/api\/v1\/me\/queue/, '');
		const problem = (status: number, code: string) =>
			route.fulfill({ status, json: { type: 'about:blank', title: '', status, code } });
		if (path === '' && request.method() === 'POST') {
			return route.fulfill({ json: queue.start(request.postDataJSON()) });
		}
		if (!queue.queue) return problem(404, 'not_found');
		if (path === '') return route.fulfill({ json: queue.snapshot() });
		if (path === '/rows') return route.fulfill({ json: queue.rows(url.searchParams) });
		const tracks = queue.lists.get(path.replace('/lists/', ''));
		if (!tracks) return problem(404, 'not_found');
		return route.fulfill({ json: { libraryId: library.id, tracks } });
	});
}

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

/** The account the browser is logged in as. */
const owner = { id: '1', username: 'sam', displayName: 'Sam', role: 'owner', hasAvatar: false };

const pageOf = <T>(items: T[]) => ({ items, total: items.length, nextCursor: null });

/**
 * Serves the library by intercepting `/api/v1`, with this browser logged in as the owner and no
 * playlists. Returns every play reported to it.
 */
export async function serveLibrary(page: Page) {
	const plays: PlayReport[] = [];
	await page.route('**/api/v1/**', async (route: Route) => {
		const url = new URL(route.request().url());
		const path = url.pathname.replace(/^.*\/api\/v1/, '');
		const lib = `/libraries/${library.id}`;
		const json = (body: unknown) => route.fulfill({ json: body });
		const problem = (status: number, code: string, detail?: string) =>
			route.fulfill({ status, json: { type: 'about:blank', title: '', status, code, detail } });

		if (path === '/server') return json({ version: 'test', apiVersion: '1', setupRequired: false });
		if (path === '/me/plays') {
			plays.push(...route.request().postDataJSON().items);
			return route.fulfill({ status: 204 });
		}
		if (path === '/me/plugins') return json({ items: [] });
		if (path === '/me') return json(owner);

		if (path === '/libraries') return json({ items: [library] });
		if (path === `${lib}/albums`)
			return json(pageOf(tagged([album], filterOf(url.searchParams), (a) => a.tags)));
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
			// A lookup by IDs: the tracks held, in the order named.
			const named = url.searchParams.get('ids')!.split(',');
			return json(pageOf(named.flatMap((id) => tracks.filter((t) => t.id === id))));
		}
		if (path === `${lib}/tracks`) {
			const listed = tagged(
				sortedAsAsked(url.searchParams),
				filterOf(url.searchParams),
				(t) => t.tags
			);
			return json(pageOf(listed));
		}
		if (path === `${lib}/tag-types`) {
			return json([
				{ key: 'genre', tagCount: 2 },
				{ key: 'mood', tagCount: 1 }
			]);
		}
		if (path === `${lib}/tags`) {
			const type = url.searchParams.get('type');
			const q = url.searchParams.get('q')?.trim().toLowerCase();
			const of = TAGS.filter(
				(t) => (!type || t.type === type) && (!q || t.name.toLowerCase().includes(q))
			);
			return json(pageOf(of.toSorted((a, b) => b.trackCount - a.trackCount)));
		}
		const tag = TAGS.find((t) => path === `${lib}/tags/${t.id}`);
		if (tag) return json(tag);
		if (path === `${lib}/artists`) {
			const all = [{ ...artist, image: null, albumCount: 1, trackCount: 3 }];
			return json(pageOf(tagged(all, filterOf(url.searchParams), () => byType(TAGS))));
		}
		if (path === `${lib}/artists/${artist.id}`) {
			return json({
				...artist,
				image: null,
				albumCount: 1,
				trackCount: 3,
				addedAt: '2026-01-01T00:00:00.000Z',
				biography: null,
				tags: byType(TAGS),
				appearanceCount: 0
			});
		}
		if (path.startsWith(`${lib}/playlists`)) return json(pageOf([]));
		if (path === `${lib}/recent-searches`) return json({ items: [] });
		if (path === `${lib}/search`) {
			const q = url.searchParams.get('q') ?? '';
			const hits = tagged(
				tracks.filter((t) => t.title.toLowerCase().includes(q.toLowerCase())),
				filterOf(url.searchParams),
				(t) => t.tags
			);
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
			return json({ ...one, lyrics: 'none', loudness: null, identifiers: {} });
		}
		const media = /^\/libraries\/\d+\/tracks\/(\d+)\/(playback|waveform|audio)$/.exec(path);
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
	// After the rest, so it is asked first for the queue's paths.
	await serveQueue(page, queueOf(page));
	return { plays };
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
	// The phone plays the second item of the account's queue, which this device loads on hello.
	queueOf(page).start({
		queueId: 'phone-queue',
		context: { type: 'library' },
		source: { key: 'phone', libraryId: library.id, tracks: tracks.map((t) => t.id) },
		start: 1
	} as StartQueue);
	let state: PlayerState = {
		status: 'playing',
		activeDeviceId: phone.id,
		itemId: 'phone.1',
		positionMs: 0,
		positionAt: now(),
		volume: 1
	};
	await page.routeWebSocket('**/api/v1/realtime', (ws) => {
		const send = (message: RealtimeServerMessage) => ws.send(JSON.stringify(message));
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
			// As the server does: an item the account's queue lacks is refused, with the state as it is.
			const queue = queueOf(page).queue;
			const holds =
				!!report &&
				!!queue &&
				(JSON.parse(queue.currentRow() ?? '{}').item === report.itemId ||
					queue.findItem(report.itemId) !== undefined);
			if (report && !holds) send({ type: 'player', state });
			else if (report && (starts || deviceId === state.activeDeviceId)) {
				state = {
					...state,
					status: report.status === 'buffering' ? 'paused' : report.status,
					activeDeviceId: report.status === 'stopped' ? null : deviceId,
					itemId: report.itemId,
					positionMs: report.positionMs,
					positionAt: now(),
					volume: report.volume ?? (deviceId === state.activeDeviceId ? state.volume : null)
				};
				send({ type: 'player', state });
			}
		});
	});
	return { sent, state: () => state };
}
