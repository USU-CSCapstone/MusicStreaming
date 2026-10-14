import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Playlist } from './api/types';
import { deleteWithUndo, deleted } from './deletions.svelte';
import { TOAST_MS, toast, toaster } from './toast.svelte';

const playlist = { id: 'p1', libraryId: '11', title: 'Road Trip' } as Playlist;

let requests: { method: string; path: string }[];
let status: number;
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
	status = 204;
	deleted.clear();
});

afterEach(() => {
	toaster.dismiss();
	vi.useRealTimers();
});

describe('deleteWithUndo', () => {
	it('hides the playlist at once and offers Undo, sending nothing yet', () => {
		deleteWithUndo(fetch, playlist);
		expect(deleted.has('p1')).toBe(true);
		expect(toaster.current?.message).toBe('Deleted “Road Trip”');
		expect(toaster.current?.action?.label).toBe('Undo');
		expect(requests).toEqual([]);
	});

	it('brings it back on Undo, and never sends', async () => {
		deleteWithUndo(fetch, playlist);
		toaster.act();
		await vi.advanceTimersByTimeAsync(TOAST_MS * 2);
		expect(deleted.has('p1')).toBe(false);
		expect(requests).toEqual([]);
	});

	it('sends the deletion once the toast times out', async () => {
		deleteWithUndo(fetch, playlist);
		await vi.advanceTimersByTimeAsync(TOAST_MS);
		expect(requests).toEqual([{ method: 'DELETE', path: '/libraries/11/playlists/p1' }]);
		expect(deleted.has('p1')).toBe(true);
	});

	it('sends it when the toast is dismissed or replaced', async () => {
		deleteWithUndo(fetch, playlist);
		toaster.dismiss();
		deleteWithUndo(fetch, { ...playlist, id: 'p2' });
		toast('Something else');
		await settled();
		expect(requests.map((r) => r.path)).toEqual([
			'/libraries/11/playlists/p1',
			'/libraries/11/playlists/p2'
		]);
	});

	it('waits for pending edits before sending', async () => {
		let finish!: () => void;
		const edits = new Promise<void>((r) => (finish = r));
		deleteWithUndo(fetch, playlist, () => edits);
		toaster.dismiss();
		await settled();
		expect(requests).toEqual([]);
		finish();
		await settled();
		expect(requests).toHaveLength(1);
	});

	it('brings it back, and says so, when the deletion fails', async () => {
		status = 500;
		deleteWithUndo(fetch, playlist);
		toaster.dismiss();
		await settled();
		expect(deleted.has('p1')).toBe(false);
		expect(toaster.current?.message).toBe('Couldn’t delete “Road Trip”. Try later.');
	});

	it('counts a playlist already gone as deleted', async () => {
		status = 404;
		deleteWithUndo(fetch, playlist);
		toaster.dismiss();
		await settled();
		expect(deleted.has('p1')).toBe(true);
		expect(toaster.current).toBeNull();
	});
});
