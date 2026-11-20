import { page } from 'vitest/browser';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type {
	AlbumSummary,
	ArtistSummary,
	PlayContext,
	Playlist,
	Tag,
	TrackSummary
} from '$lib/api/types';

vi.mock('$app/state', () => ({
	page: { data: { library: { id: '1' }, tagTypes: [{ key: 'genre', tagCount: 3 }] } }
}));
const fake = vi.hoisted(() => ({
	plays: [] as unknown[][],
	listed: [] as Record<string, unknown>[],
	tracks: (async () => ({ items: [], nextCursor: null, total: 0 })) as () => Promise<unknown>,
	items: (async () => ({ items: [], nextCursor: null, total: 0 })) as () => Promise<unknown>
}));
vi.mock('$lib/player.svelte', () => ({
	player: { play: (...args: unknown[]) => void fake.plays.push(args) }
}));
vi.mock('$lib/api/client', async (actual) => ({
	...(await actual<typeof import('$lib/api/client')>()),
	listTracks: (_f: unknown, _l: string, query: Record<string, unknown>) => {
		fake.listed.push(query);
		return fake.tracks();
	},
	listPlaylistItems: (_f: unknown, _l: string, id: string) => {
		fake.listed.push({ playlistId: id });
		return fake.items();
	}
}));

const { toaster } = await import('$lib/toast.svelte');
const { default: TopResult } = await import('./TopResult.svelte');

const track = (id: string, title: string) =>
	({
		id,
		title,
		artists: [{ id: '21', name: 'Weezer' }],
		album: { id: '31', title: 'Blue Album', artists: [], image: null },
		availability: 'available',
		liked: false
	}) as unknown as TrackSummary;
const album = {
	id: '31',
	title: 'Blue Album',
	artists: [{ id: '21', name: 'Weezer' }],
	releaseDate: '1994-05-10',
	image: null,
	trackCount: 10
} as unknown as AlbumSummary;
const artist = {
	id: '21',
	name: 'Weezer',
	image: null,
	albumCount: 3,
	trackCount: 30
} as unknown as ArtistSummary;

const context: PlayContext = { type: 'search', query: 'say it', section: 'tracks' };
const from = { recipe: { type: 'search' as const, q: 'say it' }, total: 4 };

beforeEach(() => {
	fake.plays.length = 0;
	fake.listed.length = 0;
	fake.tracks = async () => ({ items: [], nextCursor: null, total: 0 });
	fake.items = async () => ({ items: [], nextCursor: null, total: 0 });
});

afterEach(() => toaster.dismiss());

const playlist = {
	id: '61',
	kind: 'saved',
	title: 'Road Trip',
	image: null,
	trackCount: 3
} as unknown as Playlist;
const tag = {
	id: '30',
	type: 'genre',
	name: 'Rock',
	trackCount: 12,
	albumCount: 2,
	artistCount: 1
} as unknown as Tag;

describe('TopResult.svelte', () => {
	it('plays a track first of the search’s songs, and keeps the search', async () => {
		const acted = vi.fn();
		const song = track('5', "Say It Ain't So");
		render(TopResult, {
			props: { result: { type: 'track', track: song }, context, from, onact: acted }
		});

		await expect.element(page.getByText('Song', { exact: true })).toBeVisible();
		await expect.element(page.getByText("Say It Ain't So")).toBeVisible();
		await expect.element(page.getByRole('link', { name: 'Blue Album' })).toBeVisible();
		await page.getByRole('button', { name: "Play Say It Ain't So" }).click();

		expect(fake.plays).toEqual([[[song], 0, context, from]]);
		expect(acted).toHaveBeenCalledWith({ type: 'track', id: '5' });
	});

	it('shows a lyric with its line, and plays its song among the lyric matches', async () => {
		const acted = vi.fn();
		const [before, song] = [track('4', 'Buddy Holly'), track('5', "Say It Ain't So")];
		const lyric = { track: song, line: { text: "Say it ain't so", startMs: 1200 }, before: [], after: [] };
		const lyricContext: PlayContext = { type: 'search', query: 'say it', section: 'lyrics' };
		const lyrics = { tracks: [before, song], context: lyricContext };
		render(TopResult, {
			props: { result: { type: 'lyric', lyric }, context, from, lyrics, onact: acted }
		});

		await expect.element(page.getByText('Lyrics', { exact: true })).toBeVisible();
		await expect.element(page.getByText("“Say it ain't so”")).toBeVisible();
		await page.getByRole('button', { name: "Play Say It Ain't So" }).click();

		// From the song's start, among the matches, not the search's songs.
		expect(fake.plays).toEqual([[[before, song], 1, lyricContext]]);
		expect(acted).toHaveBeenCalledWith({ type: 'track', id: '5' });
	});

	it('plays an album from its first song, the server resolving the rest', async () => {
		const acted = vi.fn();
		const first = [track('7', 'My Name Is Jonas'), track('8', 'No One Else')];
		fake.tracks = async () => ({ items: first, nextCursor: 'next', total: 10 });
		render(TopResult, { props: { result: { type: 'album', album }, context, from, onact: acted } });

		await expect.element(page.getByText('Album', { exact: true })).toBeVisible();
		await expect.element(page.getByRole('link', { name: 'Blue Album' })).toBeVisible();
		await page.getByRole('button', { name: 'Play Blue Album' }).click();

		await vi.waitFor(() => expect(fake.plays).toHaveLength(1));
		expect(fake.listed).toEqual([{ albumId: '31', sort: 'album' }]);
		const [tracks, index, played, by] = fake.plays[0];
		expect([tracks, index, played]).toEqual([first, 0, { type: 'album', id: '31' }]);
		expect(by).toEqual({
			recipe: expect.objectContaining({ type: 'album', albumId: '31' }),
			total: 10
		});
		expect(acted).toHaveBeenCalledWith({ type: 'album', id: '31' });
	});

	it('plays an artist as their page does', async () => {
		const acted = vi.fn();
		fake.tracks = async () => ({ items: [track('7', 'Buddy Holly')], nextCursor: null, total: 1 });
		render(TopResult, {
			props: { result: { type: 'artist', artist }, context, from, onact: acted }
		});

		await expect.element(page.getByText('Artist', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Play Weezer' }).click();

		await vi.waitFor(() => expect(fake.plays).toHaveLength(1));
		expect(fake.listed).toEqual([{ artistId: '21', sort: 'album' }]);
		expect(fake.plays[0][2]).toEqual({ type: 'artist', id: '21', scope: 'all' });
		expect(acted).toHaveBeenCalledWith({ type: 'artist', id: '21' });
	});

	it('says so when an album’s songs cannot be read, and keeps nothing', async () => {
		const acted = vi.fn();
		fake.tracks = async () => {
			throw new Error('offline');
		};
		render(TopResult, { props: { result: { type: 'album', album }, context, from, onact: acted } });

		await page.getByRole('button', { name: 'Play Blue Album' }).click();

		await vi.waitFor(() => expect(toaster.current?.message).toBe("Couldn't play Blue Album."));
		expect(fake.plays).toEqual([]);
		expect(acted).not.toHaveBeenCalled();
	});

	it('offers the actions for what it is', async () => {
		render(TopResult, {
			props: { result: { type: 'album', album }, context, from, onact: vi.fn() }
		});
		await expect
			.element(page.getByRole('button', { name: 'Actions for “Blue Album”' }))
			.toBeVisible();
	});

	it('plays a playlist from its first song, leaving out what was hidden, and keeps the search', async () => {
		const acted = vi.fn();
		const [one, hidden, two] = [track('7', 'One'), track('8', 'Gone'), track('9', 'Two')];
		(hidden as { hidden?: true }).hidden = true;
		fake.items = async () => ({
			items: [{ track: one }, { track: hidden }, { track: two }],
			nextCursor: null,
			total: 3
		});
		render(TopResult, {
			props: { result: { type: 'playlist', playlist }, context, from, onact: acted }
		});

		await expect.element(page.getByText('Playlist', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Play Road Trip' }).click();

		await vi.waitFor(() => expect(fake.plays).toHaveLength(1));
		const [tracks, index, played, by] = fake.plays[0];
		expect([tracks, index, played]).toEqual([[one, two], 0, { type: 'playlist', id: '61' }]);
		expect(by).toEqual({ recipe: { type: 'playlist', playlistId: '61' }, total: 3 });
		// A recent search names a song, an album or an artist, so it is kept with none.
		expect(acted).toHaveBeenCalledWith(undefined);
	});

	it('plays a tag’s songs album by album, named for its type', async () => {
		const acted = vi.fn();
		fake.tracks = async () => ({ items: [track('7', 'Buddy Holly')], nextCursor: null, total: 12 });
		render(TopResult, { props: { result: { type: 'tag', tag }, context, from, onact: acted } });

		await expect.element(page.getByText('Genre', { exact: true })).toBeVisible();
		await expect.element(page.getByRole('link', { name: 'Rock' })).toBeVisible();
		await page.getByRole('button', { name: 'Play Rock' }).click();

		await vi.waitFor(() => expect(fake.plays).toHaveLength(1));
		expect(fake.listed).toEqual([{ tagAll: '30', sort: 'album' }]);
		expect(fake.plays[0].slice(2)).toEqual([
			{ type: 'tag', id: '30' },
			{
				recipe: { type: 'library', filters: { tagAll: ['30'] }, sort: 'album', order: 'asc' },
				total: 12
			}
		]);
		expect(acted).toHaveBeenCalledWith(undefined);
	});
});
