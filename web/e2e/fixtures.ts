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
	RecentSearch,
	Session,
	SetupRequest,
	TrackSummary
} from '../src/lib/api/types';
import { withManifest } from '../../tools/plugin-pack/manifest.mjs';
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

const artist = { id: '21', name: 'Aurora Lane' };

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

const track = (id: string, title: string, disc: number, n: number): TrackSummary => ({
	id,
	title,
	artists: [artist],
	album: { id: album.id, title: album.title, artists: [artist], image: null },
	discNumber: disc,
	trackNumber: n,
	durationUs: 2_000_000,
	explicit: false,
	audio: { codec: 'pcm', lossless: true },
	availability: 'available'
});

export const tracks = [
	track('41', 'Signal Part 1', 1, 1),
	track('42', 'Signal Part 2', 1, 2),
	track('43', 'Signal Remix', 2, 1)
];

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

/** The session a successful setup or login returns. */
const session = (request: SetupRequest): Session => ({
	token: 'token',
	user: {
		id: '1',
		username: request.username,
		displayName: request.displayName ?? request.username,
		role: 'owner',
		hasAvatar: false
	},
	device: {
		id: '2',
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
	apiVersion: '0.2',
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
		role = 'owner' as 'owner' | 'admin' | 'user'
	} = {}
) {
	// Plugin administration runs through the mock's own logic, on a fresh directory per test.
	const plugins = new PluginStore(mkdtempSync(join(tmpdir(), 'jc-e2e-plugins-')), () =>
		empty ? [] : [library.id]
	);
	const plays: PlayReport[] = [];
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
	await page.route('**/api/v1/**', async (route: Route) => {
		const url = new URL(route.request().url());
		const path = url.pathname.replace(/^.*\/api\/v1/, '');
		const lib = `/libraries/${library.id}`;
		const json = (body: unknown) => route.fulfill({ json: body });
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
			if (!out) return route.fulfill({ status: 404 });
			return out.body === undefined
				? route.fulfill({ status: out.status })
				: route.fulfill({ status: out.status, json: out.body });
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
		if (path === `${lib}/tracks`) return json(pageOf(tracks));
		if (path === `${lib}/artists`) {
			return json(pageOf([{ ...artist, image: null, albumCount: 1, trackCount: 3 }]));
		}
		if (path === `${lib}/playlists`) return json(pageOf([]));
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
