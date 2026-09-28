// A tiny library served by intercepting /api/v1, so the tests need no music folder.

import type { Page, Route } from '@playwright/test';
import type { AlbumSummary, Library, TrackSummary } from '../src/lib/api/types';

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

const pageOf = <T>(items: T[]) => ({ items, total: items.length, nextCursor: null });

export async function serveLibrary(page: Page, { empty = false } = {}) {
	await page.route('**/api/v1/**', async (route: Route) => {
		const url = new URL(route.request().url());
		const path = url.pathname.replace(/^.*\/api\/v1/, '');
		const lib = `/libraries/${library.id}`;
		const json = (body: unknown) => route.fulfill({ json: body });

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
		return route.fulfill({
			status: 404,
			json: { type: 'about:blank', title: 'Not Found', status: 404, code: 'not_found' }
		});
	});
}
