import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Adding } from './adding.svelte';
import type { SentEdit, TrackSummary } from './api/types';

const fake = vi.hoisted(() => ({
	queue: { queueId: 'q1' as string | null, edit: (() => {}) as (edit: SentEdit) => void }
}));
vi.mock('$lib/queue/client.svelte', () => ({ queue: fake.queue }));

const { addToQueue, placing, startPlacing } = await import('./queueing.svelte');
const { panels } = await import('./panels.svelte');
const { toaster } = await import('./toast.svelte');

const track = (id: string) => ({ id }) as TrackSummary;

/** Songs the device holds: they go as `tracks`. */
const songs = (ids: string[]): Adding => ({
	label: `${ids.length} songs`,
	count: ids.length,
	refs: [],
	tracks: async () => ids.map(track)
});

const album: Adding = {
	...songs([]),
	label: '“Signal”',
	count: 3,
	recipe: { type: 'album', albumId: '31', sort: 'album.asc' }
};

let edits: SentEdit[];

beforeEach(() => {
	edits = [];
	fake.queue.queueId = 'q1';
	fake.queue.edit = (e) => void edits.push(e);
	placing.what = null;
	panels.queue = false;
});

afterEach(() => toaster.dismiss());

describe('addToQueue', () => {
	it('sends the tracks held at the front for Queue Next, and says so', async () => {
		expect(await addToQueue(songs(['4', '5']), '11', { type: 'manualStart' })).toBe(true);
		expect(edits).toEqual([
			{
				type: 'add',
				source: { key: expect.any(String), libraryId: '11', tracks: ['4', '5'] },
				before: { type: 'manualStart' }
			}
		]);
		expect(toaster.current?.message).toBe('2 songs queued next');
	});

	it('sends a collection by its recipe, never its tracks, at the end for Queue Last', async () => {
		const tracks = vi.fn(async () => [track('1')]);
		await addToQueue({ ...album, tracks }, '11', { type: 'manualEnd' });
		expect(edits[0]).toEqual({
			type: 'add',
			source: {
				key: expect.any(String),
				libraryId: '11',
				recipe: { type: 'album', albumId: '31', sort: 'album.asc' }
			},
			before: { type: 'manualEnd' }
		});
		expect(tracks).not.toHaveBeenCalled();
		expect(toaster.current?.message).toBe('“Signal” queued last');
	});

	it('mints a key for each add', async () => {
		await addToQueue(songs(['4']), '11', { type: 'manualEnd' });
		await addToQueue(songs(['4']), '11', { type: 'manualEnd' });
		const [a, b] = edits.map((e) => (e.type === 'add' ? e.source.key : ''));
		expect(a).not.toBe(b);
	});

	it('says to start something first when there is no queue, and sends nothing', async () => {
		fake.queue.queueId = null;
		expect(await addToQueue(songs(['4']), '11', { type: 'manualEnd' })).toBe(false);
		expect(edits).toEqual([]);
		expect(toaster.current?.message).toBe('Start something playing first, then queue more.');
	});

	it('sends nothing for a collection with no songs', async () => {
		expect(await addToQueue(songs([]), '11', { type: 'manualEnd' })).toBe(false);
		expect(edits).toEqual([]);
		expect(toaster.current?.message).toBe('There are no songs in 0 songs to queue.');
	});

	it('says when the songs could not be fetched', async () => {
		const failing = { ...songs(['4']), tracks: async () => Promise.reject(new Error('down')) };
		expect(await addToQueue(failing, '11', { type: 'manualEnd' })).toBe(false);
		expect(edits).toEqual([]);
		expect(toaster.current?.message).toBe('Couldn’t add 1 songs to the queue.');
	});
});

describe('startPlacing', () => {
	it('opens the queue panel with what is to be placed', () => {
		startPlacing(album, '11');
		expect(placing).toMatchObject({ what: album, libraryId: '11' });
		expect(panels.queue).toBe(true);
	});

	it('does nothing but say so when there is no queue', () => {
		fake.queue.queueId = null;
		startPlacing(album, '11');
		expect(placing.what).toBeNull();
		expect(panels.queue).toBe(false);
		expect(toaster.current?.message).toBe('Start something playing first, then queue more.');
	});
});
