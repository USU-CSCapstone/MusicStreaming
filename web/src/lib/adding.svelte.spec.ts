import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { addTo, alreadyIn, cardAdding, newPlaylist, songs } from './adding.svelte';
import { catalog } from './catalog/client.svelte';
import { connectivity } from './connectivity.svelte';
import { sender } from './sync/sender';
import { memoryEnv } from './sync/store';
import type { Playlist, TrackSummary } from './api/types';
import { toaster } from './toast.svelte';
import type { Card } from './cards';

const card = (type: Card['type']): Card => ({
	type,
	count: 7,
	id: '5',
	href: '/x',
	title: 'Signal',
	subtitle: '',
	image: null,
	icon: type,
	round: false
});

describe('cardAdding', () => {
	it('names an album and an artist by the recipe the server resolves, in album order', () => {
		expect(cardAdding(fetch, '11', card('album'))).toMatchObject({
			label: '“Signal”',
			count: 7,
			refs: [{ type: 'album', id: '5' }],
			recipe: { type: 'album', albumId: '5', sort: 'album', order: 'asc' }
		});
		expect(cardAdding(fetch, '11', card('artist')).recipe).toEqual({
			type: 'artist',
			artistId: '5',
			scope: 'all',
			artistCredit: 'any',
			sort: 'album',
			order: 'asc'
		});
	});

	it('names a playlist by its recipe, queued by the count the card has and with no fetch', async () => {
		const fetched: string[] = [];
		const spy = ((input: RequestInfo | URL) => {
			fetched.push(String(input));
			return Promise.reject(new Error('no fetch expected'));
		}) as typeof fetch;
		const adding = cardAdding(spy, '11', card('playlist'));
		expect(adding).toMatchObject({
			label: '“Signal”',
			count: 7,
			refs: [{ type: 'playlist', id: '5' }],
			recipe: { type: 'playlist', playlistId: '5' }
		});
		// No sort or order is sent for the playlist's own order, and no version, which is the server's.
		expect(adding.recipe).toEqual({ type: 'playlist', playlistId: '5' });
		expect(fetched).toEqual([]);
	});
});

describe('adding to a playlist offline', () => {
	const playlist = {
		id: 'p1',
		libraryId: '11',
		title: 'Road Trip',
		kind: 'saved',
		version: 1
	} as Playlist;
	const what = songs([{ id: '9', title: 'Signal' } as TrackSummary]);
	afterEach(() => {
		sender.stop();
		connectivity.setWorkOffline(false);
		toaster.dismiss();
	});

	it('gives no duplicate warning where the server cannot be asked, and keeps the add', async () => {
		// The network is up and the server down.
		const down = (() => Promise.reject(new TypeError('Failed to fetch'))) as typeof fetch;
		expect(await alreadyIn(down, playlist, what)).toEqual([]);
		connectivity.reach();
		connectivity.setWorkOffline(true);
		sender.start('1', { env: memoryEnv() });
		expect(await alreadyIn(fetch, playlist, what)).toEqual([]);
		await addTo(fetch, playlist, what);
		await sender.idle();
		expect(await sender.pending()).toBe(1);
		expect(toaster.current?.message).toBe('Added “Signal” to “Road Trip”');
	});

	it('still fails a duplicate check the server answered with a refusal', async () => {
		const refuse = (async () =>
			new Response(JSON.stringify({ status: 422, code: 'validation_failed' }), {
				status: 422
			})) as typeof fetch;
		await expect(alreadyIn(refuse, playlist, what)).rejects.toThrow();
	});
});

describe('with the library on the device', () => {
	const playlist = { id: 'p1', libraryId: '11', title: 'Road Trip', kind: 'saved' } as Playlist;
	const device = {
		apply: vi.fn(async () => {}),
		rebase: async () => {},
		confirmed: async () => {}
	};
	/** What a request for the playlist's items is answered with. */
	let held: string[];
	const reader = (async () => {
		throw new Error('the server is not asked');
	}) as unknown as typeof fetch;

	beforeEach(() => {
		held = ['9'];
		vi.spyOn(catalog, 'onDevice').mockReturnValue(true);
		vi.spyOn(catalog, 'waitForDevice').mockResolvedValue(true);
		vi.spyOn(catalog, 'query').mockImplementation(async () => {
			const items = held.map((id) => ({ track: { id }, hidden: false }));
			return JSON.stringify({ items, nextCursor: null, total: items.length, version: 1 });
		});
		sender.start('1', { env: memoryEnv(), device });
	});
	afterEach(() => {
		vi.restoreAllMocks();
		sender.stop();
		connectivity.setWorkOffline(false);
		toaster.dismiss();
	});

	it('warns of a song already in the playlist offline, from the device and not the server', async () => {
		connectivity.setWorkOffline(true);
		const what = songs([9, 8, 9].map((id) => ({ id: String(id), title: 'S' }) as TrackSummary));
		expect(await alreadyIn(reader, playlist, what)).toEqual(['9']);
		expect(
			await alreadyIn(reader, playlist, songs([{ id: '4', title: 'S' } as TrackSummary]))
		).toEqual([]);
	});

	it('makes a playlist with the songs on the device, which can be opened at once offline', async () => {
		connectivity.setWorkOffline(true);
		const what = songs([{ id: '9', title: 'Signal' } as TrackSummary]);
		const { id, open } = await newPlaylist('11', { title: 'New', description: null }, what);
		expect(open).toBe(true);
		expect(id).toMatch(/^[1-9]\d{0,18}$/);
		expect(device.apply).toHaveBeenCalledWith(
			'1',
			expect.objectContaining({
				library: '11',
				calls: [
					expect.objectContaining({
						name: 'createPlaylist',
						args: expect.objectContaining({ id, items: ['9'] })
					})
				]
			}),
			false
		);
		expect(await sender.pending()).toBe(1);
	});
});

describe('a playlist made where the device does not hold the library', () => {
	afterEach(() => {
		sender.stop();
		connectivity.setWorkOffline(false);
		// A server that could not be reached in one spec is not the next one's.
		connectivity.reach();
		toaster.dismiss();
	});

	it('is left for when the device is online, and says so, if it cannot be opened', async () => {
		connectivity.setWorkOffline(true);
		sender.start('1', { env: memoryEnv() });
		const { open } = await newPlaylist('11', { title: 'New', description: null });
		expect(open).toBe(false);
		expect(toaster.current?.message).toBe(
			'“New” will be created as soon as the server can be reached.'
		);
		expect(await sender.pending()).toBe(1);
	});

	it('is left for later, and says so, when the server cannot be reached', async () => {
		vi.useFakeTimers();
		try {
			const down = (async () => {
				throw new TypeError('Failed to fetch');
			}) as typeof fetch;
			sender.start('1', { env: memoryEnv(), fetch: down });
			const made = newPlaylist('11', { title: 'New', description: null });
			await vi.advanceTimersByTimeAsync(3100);
			expect((await made).open).toBe(false);
			expect(toaster.current?.message).toBe(
				'“New” will be created as soon as the server can be reached.'
			);
		} finally {
			vi.useRealTimers();
		}
	});

	it('is opened once the server has it', async () => {
		const sent: string[] = [];
		const ok = (async (_input: RequestInfo | URL, init: RequestInit = {}) => {
			sent.push(String(init.method));
			return Response.json({ id: '1', version: 1 }, { status: 201 });
		}) as typeof fetch;
		sender.start('1', { env: memoryEnv(), fetch: ok });
		const { open } = await newPlaylist('11', { title: 'New', description: null });
		expect(open).toBe(true);
		expect(sent).toEqual(['POST']);
	});

	it('is opened once the server has it, though it answered before the device took the change', async () => {
		const ok = (async () =>
			Response.json({ id: '1', version: 1 }, { status: 201 })) as typeof fetch;
		const slow = {
			apply: () => new Promise<void>((done) => setTimeout(done, 100)),
			rebase: async () => {},
			confirmed: async () => {}
		};
		sender.start('1', { env: memoryEnv(), fetch: ok, device: slow });
		const { open } = await newPlaylist('11', { title: 'New', description: null });
		expect(open).toBe(true);
		expect(toaster.current).toBeNull();
	});
});
