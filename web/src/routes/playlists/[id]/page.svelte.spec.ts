import { page } from 'vitest/browser';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { Playlist, PlaylistItem, TrackSummary } from '$lib/api/types';

vi.mock('$app/state', () => ({
	page: { url: new URL('https://jewelcase.test/playlists/p1'), data: { tagTypes: [] } }
}));
vi.mock('$lib/catalog/client.svelte', () => ({
	catalog: {
		onDevice: () => true,
		onChanged: () => () => {},
		changes: 0,
		changedSince: () => false,
		query: async () => '{}',
		waitForDevice: async () => true
	}
}));

const { sender } = await import('$lib/sync/sender');
const { memoryEnv } = await import('$lib/sync/store');
const { connectivity } = await import('$lib/connectivity.svelte');
const { EditablePlaylist } = await import('$lib/playlist.svelte');
const { default: Playlist } = await import('./+page.svelte');

/** Track number `n` of the album Signal. */
const track = (n: number): TrackSummary =>
	({
		id: String(n),
		title: `Track ${n}`,
		artists: [{ id: '21', name: 'Aurora Lane' }],
		album: { id: '31', title: 'Signal', artists: [], image: null },
		discNumber: 1,
		trackNumber: n,
		durationUs: 2_000_000,
		releaseDate: null,
		tags: {},
		explicit: false,
		audio: { codec: 'flac', lossless: true },
		availability: 'available',
		sortKeys: { title: `track ${n}`, artist: '', album: 'signal' },
		liked: false
	}) as TrackSummary;
/** Track `n` as a playlist item. */
const item = (n: number): PlaylistItem => ({
	track: track(n),
	addedAt: '2026-01-01T00:00:00.000Z'
});
const playlist = {
	id: 'p1',
	libraryId: '11',
	title: 'Road Trip',
	description: null,
	image: null,
	kind: 'saved',
	trackCount: 2,
	durationUs: 4_000_000,
	pinned: false,
	version: 1
} as unknown as Playlist;

beforeEach(() => connectivity.setWorkOffline(true));
afterEach(() => {
	sender.stop();
	connectivity.setWorkOffline(false);
});

it('shows the playlist as the device has it, an edit made offline once, not again over it', async () => {
	const device = { apply: async () => {}, rebase: async () => {}, confirmed: async () => {} };
	sender.start('1', { env: memoryEnv(), device });
	// An edit made on an earlier visit, which the device's copy holds, still to be sent.
	const before = new EditablePlaylist(fetch, { playlist, items: [item(1), item(2)] });
	before.edit({ ops: [{ op: 'add', items: [{ type: 'track', id: '9' }] }] }, [track(9)]);
	await sender.idle();

	render(Playlist, {
		data: {
			lib: '11',
			loaded: {
				playlist: { ...playlist, version: 2, trackCount: 3 },
				items: [item(1), item(2), item(9)]
			}
		}
	} as never);
	await expect.element(page.getByText('Track 9')).toBeVisible();
	await expect.element(page.getByText('3 songs')).toBeVisible();
	expect(page.getByText('Track 9').elements()).toHaveLength(1);
});
