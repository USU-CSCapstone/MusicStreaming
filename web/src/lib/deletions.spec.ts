import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Playlist } from './api/types';
import { connectivity } from './connectivity.svelte';
import { HOLD_MS, deleteWithUndo, deleted } from './deletions.svelte';
import { marks } from './marks.svelte';
import type { Device } from './sync/device';
import type { Local } from './sync/mutation';
import { sender } from './sync/sender';
import { memoryEnv } from './sync/store';
import { TOAST_MS, toast, toaster } from './toast.svelte';

const playlist = { id: 'p1', libraryId: '11', title: 'Road Trip' } as Playlist;

let requests: { method: string; path: string }[];
let status: number;
let made: Local[];
let rebased: string[];
const device: Device = {
	// What it is asked to make, not what it is told was answered.
	apply: async (_user, local, answered) => void (answered || made.push(local)),
	rebase: async (_user, library) => void rebased.push(library),
	confirmed: async () => {}
};
const fetch = (async (input: RequestInfo | URL, init: RequestInit = {}) => {
	requests.push({
		method: init.method ?? 'GET',
		path: new URL(String(input), 'http://test').pathname.replace(/^.*\/api\/v1/, '')
	});
	return status === 204
		? new Response(null, { status })
		: new Response(JSON.stringify({ status, code: 'internal', detail: 'Try later.' }), { status });
}) as typeof globalThis.fetch;

/** Lets the deletion's awaits run. */
const settled = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => {
	vi.useFakeTimers();
	requests = [];
	made = [];
	rebased = [];
	status = 204;
	deleted.clear();
	marks.clear();
	sender.start('1', { env: memoryEnv(), fetch, device });
});

afterEach(() => {
	toaster.dismiss();
	sender.stop();
	connectivity.setWorkOffline(false);
	vi.useRealTimers();
});

describe('deleteWithUndo', () => {
	it('deletes it on the device and hides it at once, offering Undo and sending nothing yet', async () => {
		deleteWithUndo(playlist);
		await settled();
		expect(made[0].calls).toMatchObject([{ name: 'deletePlaylist', args: { playlistId: 'p1' } }]);
		expect(deleted.has('p1')).toBe(true);
		expect(toaster.current?.message).toBe('Deleted “Road Trip”');
		expect(toaster.current?.action?.label).toBe('Undo');
		expect(requests).toEqual([]);
	});

	it('sends the deletion as the toast times out, and not before', async () => {
		deleteWithUndo(playlist);
		await vi.advanceTimersByTimeAsync(TOAST_MS - 100);
		expect(requests).toEqual([]);
		await vi.advanceTimersByTimeAsync(200);
		expect(requests).toEqual([{ method: 'DELETE', path: '/libraries/11/playlists/p1' }]);
		expect(deleted.has('p1')).toBe(true);
	});

	it('is sent at once when the toast closes without Undo, even if replaced', async () => {
		deleteWithUndo(playlist);
		toast('Something else');
		await settled();
		expect(requests).toEqual([{ method: 'DELETE', path: '/libraries/11/playlists/p1' }]);
	});

	it('stays held, and Undo still brings it back, while the toast is held open past the hold', async () => {
		deleteWithUndo(playlist);
		toaster.hold();
		await vi.advanceTimersByTimeAsync(HOLD_MS * 4);
		expect(requests).toEqual([]);
		expect(toaster.current?.action?.label).toBe('Undo');
		toaster.act();
		await vi.advanceTimersByTimeAsync(HOLD_MS * 4);
		expect(requests).toEqual([]);
		expect(rebased).toEqual(['11']);
		expect(deleted.has('p1')).toBe(false);
	});

	it('goes at once when a held toast is closed', async () => {
		deleteWithUndo(playlist);
		toaster.hold();
		await vi.advanceTimersByTimeAsync(HOLD_MS * 2);
		sender.wake();
		await settled();
		expect(requests).toEqual([]);
		toaster.dismiss();
		await settled();
		expect(requests).toHaveLength(1);
	});

	it('brings it back on Undo, taking the device’s deletion back, and never sends', async () => {
		deleteWithUndo(playlist);
		toaster.act();
		await vi.advanceTimersByTimeAsync(HOLD_MS * 2);
		expect(deleted.has('p1')).toBe(false);
		expect(rebased).toEqual(['11']);
		expect(requests).toEqual([]);
		expect(await sender.pending()).toBe(0);
	});

	it('holds a deletion made offline until online, and sends it then', async () => {
		connectivity.setWorkOffline(true);
		deleteWithUndo(playlist);
		await vi.advanceTimersByTimeAsync(HOLD_MS * 2);
		expect(requests).toEqual([]);
		connectivity.setWorkOffline(false);
		sender.wake();
		await settled();
		expect(requests).toHaveLength(1);
	});

	it('unlikes everything once Liked Songs is deleted and its Undo has passed, not before', async () => {
		const likedSongs = { ...playlist, id: 'p2', kind: 'liked' } as Playlist;
		const read = { id: '5', liked: true };
		deleteWithUndo(likedSongs);
		expect(marks.liked('11', read)).toBe(true);
		toaster.dismiss();
		expect(marks.liked('11', read)).toBe(false);
	});

	it('keeps Liked Songs’ hearts on Undo', async () => {
		const likedSongs = { ...playlist, id: 'p2', kind: 'liked' } as Playlist;
		deleteWithUndo(likedSongs);
		toaster.act();
		expect(marks.liked('11', { id: '5', liked: true })).toBe(true);
	});

	it('brings it back, and says so, when the server refuses the deletion', async () => {
		status = 422;
		deleteWithUndo(playlist);
		await vi.advanceTimersByTimeAsync(HOLD_MS + 100);
		expect(deleted.has('p1')).toBe(false);
		expect(toaster.current?.message).toBe('Couldn’t delete “Road Trip”. Try later.');
	});

	it('counts a playlist already gone as deleted', async () => {
		status = 404;
		deleteWithUndo(playlist);
		await vi.advanceTimersByTimeAsync(HOLD_MS + 100);
		expect(requests).toHaveLength(1);
		expect(deleted.has('p1')).toBe(true);
		expect(toaster.current).toBeNull();
	});
});
