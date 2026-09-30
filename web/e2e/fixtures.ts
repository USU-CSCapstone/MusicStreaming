// A tiny library served by intercepting /api/v1, so the tests need no music folder.

import type { Page, Route } from '@playwright/test';
import type {
	AlbumSummary,
	Library,
	Session,
	SetupRequest,
	TrackSummary
} from '../src/lib/api/types';

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

/** The password `POST /auth/login` accepts, for any username. */
export const PASSWORD = 'correct horse battery staple';

/**
 * A server whose setup is done unless `setupRequired`, when `POST /setup` completes it, and
 * with this browser logged in, as an account with `role`, unless `signedIn` is false. Until
 * then, anything but setup and login answers `401`.
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
		if (path === '/me') {
			return json({ ...session({ username: 'sam', password: '', device }).user, role });
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
}
