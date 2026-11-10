// A tiny library served by intercepting /api/v1, so the tests need no music folder.

import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Page, Route } from '@playwright/test';
import type { PluginManifest } from '../src/lib/api/plugins';
import type {
	AlbumSummary,
	Library,
	PlayReport,
	PlayerState,
	RealtimeClientMessage,
	RealtimeServerMessage,
	QueueEdit,
	SentEdit,
	StartQueue,
	Tag,
	TagsByType,
	TrackSummary
} from '../src/lib/api/types';
import '../core-node';
import { Queue } from '../src/lib/core-wasm/jewelcase_core_wasm.js';
import { PLAYLIST_FIELDS, type SortField, sortName } from '../src/lib/sorting';
import { sortRows, sortTracks } from '../src/lib/track-sort';
import { type TagFilter, filterByTags } from '../src/lib/tag-match';
import { withManifest } from '../../tools/plugin-pack/manifest.mjs';
import {
	type Generated,
	type PlaylistLibrary,
	PlaylistStore,
	playlistRoute
} from '../mock/playlists';
import { PluginStore, adminRoute } from '../mock/plugins';

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
 * The tracks a start's recipe resolves to, as the server runs it, without those the user hid
 * (`hidden`); what the fixture does not know is empty.
 */
function resolveRecipe(
	recipe: NonNullable<StartQueue['source']['recipe']>,
	playlistRows: PlaylistRows,
	hidden: readonly string[]
): TrackSummary[] {
	return resolveShown(recipe, playlistRows).filter((t) => !hidden.includes(t.id));
}

/** The tracks a recipe's view holds, hidden ones included. */
function resolveShown(
	recipe: NonNullable<StartQueue['source']['recipe']>,
	playlistRows: PlaylistRows
): TrackSummary[] {
	const { tagAll, tagAny } = 'filters' in recipe ? (recipe.filters ?? {}) : {};
	const filter = { all: tagAll ?? [], any: tagAny ?? [] };
	/** `items` that carry the recipe's tags, as the list endpoints keep them. */
	const kept = (items: TrackSummary[]) => tagged(items, filter, (t) => t.tags);
	if (recipe.type === 'playlist') {
		// The playlist's items, narrowed by the tags' real types, then in the core's order.
		const rows = tagged(playlistRows(recipe.playlistId), filter, (r) => r.track.tags);
		const field = PLAYLIST_FIELDS.find((field) => sortName(field) === recipe.sort);
		const sort = field && { field, order: recipe.order! };
		return sortRows(
			rows,
			sort ?? null,
			(r) => r.track,
			(r) => r.addedAt
		).map((r) => r.track);
	}
	if (recipe.type === 'search') {
		return kept(tracks.filter((t) => t.title.toLowerCase().includes(recipe.q.toLowerCase())));
	}
	return kept(sortedAsAsked(new URLSearchParams({ sort: recipe.sort, order: recipe.order })));
}

/** A playlist's items in its own order, with when each was added; empty for one the fixture lacks. */
type PlaylistRows = (playlistId: string) => { track: TrackSummary; addedAt: string }[];

/**
 * The account's queue as the server keeps it: the shared core in Node holds the state, and the
 * fixture answers the queue's endpoints for it. It sends nothing over the realtime connection.
 * One per page.
 */
class FixtureQueue {
	queue: Queue | null = null;
	queueId = '';
	version = 0;
	context: object = {};
	/** The lists held, by hash. */
	lists = new Map<string, string[]>();
	/** The starts received, as sent. */
	starts: StartQueue[] = [];
	/** The edits received, as sent. */
	edits: SentEdit[] = [];
	/** The playlists a playlist recipe resolves, set once the page's playlists are served. */
	playlistRows: PlaylistRows = () => [];
	/** The tracks the user hid, which a recipe leaves out, set once the page's library is served. */
	hiddenIds: () => string[] = () => [];
	/** While set, a start is not answered until it settles. */
	holdStarts: Promise<void> | null = null;
	/** Where `queueEdit` goes, as the realtime connection would send it; unset with none. */
	broadcast: ((message: RealtimeServerMessage) => void) | null = null;
	/** The kept queues, parked when a start or a restore replaced them: each with where its playback was left. */
	kept = new Map<string, { queue: Queue; context: object; version: number; lastActive: string }>();
	/** The progress reports the page sent over the realtime connection, as sent. */
	reports: { itemId: string; positionMs: number; start: boolean; status: string }[] = [];
	/** How long the lists and pages of rows take to arrive, as on a slow link. */
	delayMs = 0;
	/** The restores received, by queue. */
	restores: string[] = [];
	/** Where playback of the active queue is, as the server would have it from the device's reports. */
	positionMs = 0;
	/** The editIds answered, with the version each made. */
	#answered = new Map<string, number>();

	/** The queue as `GET /me/queue` answers it. */
	snapshot() {
		return {
			queueId: this.queueId,
			version: this.version,
			context: this.context,
			state: JSON.parse(this.queue!.toJson())
		};
	}

	/** Starts a queue as `POST /me/queue` does: the list is the tracks sent, or what the recipe resolves to. */
	start(body: StartQueue) {
		this.starts.push(body);
		const { key, libraryId, tracks: sent, recipe } = body.source;
		const ids =
			sent ?? resolveRecipe(recipe!, this.playlistRows, this.hiddenIds()).map((t) => t.id);
		// As the server: `start` if the list holds the track there, else its first place.
		const at = sent
			? body.start
			: ids[body.start] === body.startTrackId
				? body.start
				: ids.indexOf(body.startTrackId!);
		const list = `list-${this.starts.length}`;
		const repeat = this.queue ? JSON.parse(this.queue.toJson()).repeat : 'off';
		const state = JSON.parse(Queue.started(key, libraryId, ids.length, at, repeat).toJson());
		state.sources[0].list = list;
		this.#park();
		this.queue = Queue.fromJson(JSON.stringify(state));
		this.queueId = body.queueId;
		this.context = body.context;
		this.version = 0;
		this.lists.set(list, ids);
		this.#answered.clear();
		return { list, length: ids.length };
	}

	/** Keeps the active queue if it was adjusted, as a start or a restore does, and lets go of it if not. */
	#park() {
		if (this.queue && JSON.parse(this.queue.toJson()).adjusted) {
			const { queueId, context, version } = this;
			this.kept.set(queueId, {
				queue: this.queue,
				context,
				version,
				lastActive: new Date().toISOString()
			});
			this.broadcast?.({ type: 'queuesChanged' });
		} else this.queue?.free();
	}

	/** `GET /me/queues`: the kept queues, newest first, two to a page, with what each was playing. */
	keptPage(cursor: string | null) {
		const all = [...this.kept].reverse();
		const from = cursor ? Number(cursor) : 0;
		const items = all.slice(from, from + 2).map(([queueId, { queue, context, lastActive }]) => {
			const copy = queue.copy();
			for (const [hash, ids] of this.lists) {
				copy.attachList(hash, JSON.stringify({ libraryId: library.id, tracks: ids }));
			}
			const current = copy.currentRow();
			const entry = {
				queueId,
				context,
				lastActive,
				current: current && { libraryId: library.id, trackId: JSON.parse(current).track as string },
				upNext: copy.upNextLen(),
				history: copy.historyLen()
			};
			copy.free();
			return entry;
		});
		const end = from + 2;
		return { items, total: all.length, nextCursor: end < all.length ? String(end) : null };
	}

	/**
	 * `POST /me/queues/{id}/restore`: the kept queue is the active one, the one it replaces is kept
	 * if adjusted, and every device hears of it. Null for a queue that is not kept.
	 */
	restore(id: string) {
		const kept = this.kept.get(id);
		if (!kept) return null;
		this.restores.push(id);
		this.kept.delete(id);
		this.#park();
		this.queue = kept.queue;
		this.queueId = id;
		this.context = kept.context;
		this.version = kept.version;
		const current = JSON.parse(this.queue.currentRow() ?? '{}').item as string;
		this.broadcast?.({ type: 'queueReplaced', queueId: id });
		this.broadcast?.({ type: 'queuesChanged' });
		return { ...this.snapshot(), position: { item: current, positionMs: this.positionMs } };
	}

	/** Applies an edit as `POST /me/queue/edits` does: the version it made, or the problem code. */
	edit(body: { editId: string; queueId: string; edit: SentEdit }): number | 'queue_changed' {
		if (!this.queue || body.queueId !== this.queueId) return 'queue_changed';
		const done = this.#answered.get(body.editId);
		if (done !== undefined) return done;
		this.edits.push(body.edit);
		const applied = this.#registered(body.edit);
		this.queue.apply(JSON.stringify(applied));
		this.#answered.set(body.editId, ++this.version);
		const { queueId, version } = this;
		this.broadcast?.({ type: 'queueEdit', queueId, version, editId: body.editId, edit: applied });
		return this.version;
	}

	/** An add as the server registers it: its list held by hash, its source with the list and length. */
	#registered(edit: SentEdit): QueueEdit {
		if (edit.type !== 'add') return edit;
		const { key, libraryId, tracks: sent, recipe } = edit.source;
		const ids =
			sent ?? resolveRecipe(recipe!, this.playlistRows, this.hiddenIds()).map((t) => t.id);
		const list = `list-add-${key}`;
		this.lists.set(list, ids);
		const source = { key, libraryId, list, length: ids.length, ...(recipe ? { recipe } : {}) };
		return { ...edit, source } as QueueEdit;
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
			version: this.version,
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

/** Answers the kept queues' endpoints (`/me/queues`) for `queue`; `path` is what follows `/me/queues`. */
async function serveKept(route: Route, queue: FixtureQueue, path: string) {
	const request = route.request();
	const problem = (status: number, code: string) =>
		route.fulfill({ status, json: { type: 'about:blank', title: '', status, code } });
	if (path === '') {
		return route.fulfill({
			json: queue.keptPage(new URL(request.url()).searchParams.get('cursor'))
		});
	}
	const [, id, action] = path.split('/');
	if (request.method() === 'POST' && action === 'restore') {
		const restored = queue.restore(id);
		return restored ? route.fulfill({ json: restored }) : problem(404, 'not_found');
	}
	if (request.method() === 'DELETE') {
		if (id === queue.queueId) return problem(409, 'queue_active');
		if (!queue.kept.delete(id)) return problem(404, 'not_found');
		queue.broadcast?.({ type: 'queuesChanged' });
		return route.fulfill({ status: 204 });
	}
	return problem(404, 'not_found');
}

/** Answers the queue's endpoints for `queue`, which its worker asks. */
async function serveQueue(page: Page, queue: FixtureQueue) {
	await page.route('**/api/v1/me/queue**', async (route: Route) => {
		const request = route.request();
		const url = new URL(request.url());
		const path = url.pathname.replace(/^.*\/api\/v1\/me\/queue/, '');
		if (path.startsWith('s')) return serveKept(route, queue, path.slice(1));
		const problem = (status: number, code: string) =>
			route.fulfill({ status, json: { type: 'about:blank', title: '', status, code } });
		if (path === '' && request.method() === 'POST') {
			await queue.holdStarts;
			return route.fulfill({ json: queue.start(request.postDataJSON()) });
		}
		if (!queue.queue) return problem(404, 'not_found');
		if (path === '') return route.fulfill({ json: queue.snapshot() });
		if (path === '/edits') {
			const version = queue.edit(request.postDataJSON());
			return typeof version === 'string'
				? problem(409, version)
				: route.fulfill({ json: { version } });
		}
		if (queue.delayMs && (path === '/rows' || path.startsWith('/lists/'))) {
			await new Promise((resolve) => setTimeout(resolve, queue.delayMs));
		}
		if (path === '/rows') return route.fulfill({ json: queue.rows(url.searchParams) });
		const tracks = queue.lists.get(path.replace('/lists/', ''));
		if (!tracks) return problem(404, 'not_found');
		return route.fulfill({ json: { libraryId: library.id, tracks } });
	});
}

/** A playlist of the whole album, for tests that need one to start with. */
export const signalMix: Generated = {
	id: 'pl-signal',
	title: 'Signal Mix',
	description: 'Everything from Signal.',
	createdAt: '2026-01-01T00:00:00.000Z',
	trackIds: tracks.map((t) => BigInt(t.id))
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
	// A copy, since the store marks what it is given liked or not, and the fixtures are shared.
	track: (_, id) => {
		const found = tracks.find((t) => t.id === String(id));
		return found && { ...found };
	}
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

/** The account the browser is logged in as. */
const owner = { id: '1', username: 'sam', displayName: 'Sam', role: 'owner', hasAvatar: false };

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

/**
 * Serves the library by intercepting `/api/v1`, with this browser logged in as the owner, plus
 * the playlists in `playlists`. Returns every play reported to it.
 */
export async function serveLibrary(page: Page, { playlists = [] as Generated[] } = {}) {
	// Plugin administration runs through the mock's own logic, on a fresh directory per test.
	const plugins = new PluginStore(mkdtempSync(join(tmpdir(), 'jc-e2e-plugins-')), () => [
		library.id
	]);
	// Playlists, too, run through the mock's own store.
	const playlistStore = new PlaylistStore(playlistLibrary(playlists));
	queueOf(page).playlistRows = (id) =>
		(
			playlistStore.items(BigInt(library.id), id, new URLSearchParams({ limit: '1000' }))?.items ??
			[]
		).flatMap((i) => (i.track ? [{ track: i.track, addedAt: i.addedAt ?? '' }] : []));
	queueOf(page).hiddenIds = () => playlistStore.hiddenIds(BigInt(library.id)).map(String);
	const plays: PlayReport[] = [];
	// The tracks as this user sees them, through the store, which keeps their likes and hides:
	// copies marked liked or not, and hidden on a lookup, and in a list, without those hidden
	// (`requirements/playlists.md` §11, `requirements/conventions.md` §8).
	const libraryKey = BigInt(library.id);
	const marked = (of: TrackSummary[]) => {
		const copies = of.map((t) => ({ ...t }));
		playlistStore.markLiked(libraryKey, copies);
		playlistStore.markHidden(libraryKey, copies);
		return copies;
	};
	const seen = (of: TrackSummary[]) => {
		const hidden = playlistStore.hiddenIds(libraryKey);
		return marked(of.filter((t) => !hidden.includes(BigInt(t.id))));
	};
	await page.route('**/api/v1/**', async (route: Route) => {
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

		if (path === '/server') return json({ version: 'test', apiVersion: '1', setupRequired: false });
		if (path === '/me/plays') {
			plays.push(...route.request().postDataJSON().items);
			return route.fulfill({ status: 204 });
		}
		if (path === '/me/plugins') return json({ items: [] });
		if (path === '/me') return json(owner);
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
			return json(pageOf(marked(named.flatMap((id) => tracks.filter((t) => t.id === id)))));
		}
		if (path === `${lib}/tracks`) {
			const listed = tagged(
				sortedAsAsked(url.searchParams),
				filterOf(url.searchParams),
				(t) => t.tags
			);
			return json(pageOf(seen(listed)));
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
		if (path === `${lib}/recent-searches`) return json({ items: [] });
		if (path === `${lib}/search`) {
			const q = url.searchParams.get('q') ?? '';
			const hits = seen(
				tagged(
					tracks.filter((t) => t.title.toLowerCase().includes(q.toLowerCase())),
					filterOf(url.searchParams),
					(t) => t.tags
				)
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
		const marking = new RegExp(`^${lib}/tracks/(\\d+)/(like|hidden)$`).exec(path);
		if (marking) {
			const [method, id, liking] = [
				route.request().method(),
				BigInt(marking[1]),
				marking[2] === 'like'
			];
			if (method === 'DELETE') {
				if (liking) playlistStore.unlike(libraryKey, id);
				else playlistStore.unhide(libraryKey, id);
			} else if (
				!(liking ? playlistStore.like(libraryKey, id) : playlistStore.hide(libraryKey, id))
			) {
				return problem(404, 'not_found');
			}
			return route.fulfill({ status: 204 });
		}
		if (path === `${lib}/hidden-tracks`) {
			return json(playlistStore.hiddenTracks(libraryKey, url.searchParams));
		}
		const one = tracks.find((t) => path === `${lib}/tracks/${t.id}`);
		if (one) {
			return json({ ...marked([one])[0], lyrics: 'none', loudness: null, identifiers: {} });
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

/**
 * Opens the realtime connection as a server would for the queue alone: it says hello and tells
 * the page every edit the account's queue applies, the page's own included, as `queueEdit`.
 */
export async function serveQueueEdits(page: Page) {
	await page.routeWebSocket('**/api/v1/realtime', (ws) => {
		const send = (message: RealtimeServerMessage) => ws.send(JSON.stringify(message));
		send({ type: 'hello', deviceId, serverTime: new Date().toISOString() });
		queueOf(page).broadcast = send;
		ws.onMessage((data) => {
			const message = JSON.parse(String(data));
			if (message.type === 'progress') queueOf(page).reports.push(message.report);
		});
	});
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
