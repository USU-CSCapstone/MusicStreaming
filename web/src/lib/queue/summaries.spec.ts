import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TrackSummary } from '$lib/api/types';

const getTracks = vi.fn();
vi.mock('$lib/api/client', () => ({
	getTracks: (...args: unknown[]) => getTracks(...args)
}));

const refs: Record<string, { libraryId: string; trackId: string }> = {};
vi.mock('./client.svelte', () => ({
	queue: { track: async (item: string) => refs[item] ?? null }
}));

const { lookup } = await import('./summaries');

/** A track summary, by its ID. */
const summary = (id: string) => ({ id, title: `Track ${id}` }) as TrackSummary;

describe('lookup', () => {
	beforeEach(() => {
		getTracks.mockReset();
		for (const key of Object.keys(refs)) delete refs[key];
	});

	it('finds the tracks of items by library, only asking for those it does not know', async () => {
		Object.assign(refs, {
			'a.0': { libraryId: '1', trackId: '10' },
			'a.1': { libraryId: '1', trackId: '11' },
			'b.0': { libraryId: '2', trackId: '20' }
		});
		getTracks.mockImplementation(async (_f, _library: string, ids: string[]) => ({
			items: ids.map(summary)
		}));

		const first = await lookup(['a.0', 'a.1', 'b.0']);
		expect([...first.values()].map((t) => t?.title)).toEqual(['Track 10', 'Track 11', 'Track 20']);
		expect(getTracks.mock.calls.map(([, library, ids]) => [library, ids])).toEqual([
			['1', ['10', '11']],
			['2', ['20']]
		]);

		getTracks.mockClear();
		await lookup(['a.0']);
		expect(getTracks).not.toHaveBeenCalled();
	});

	it('maps an item the queue lacks, or a track out of reach, to null rather than failing', async () => {
		refs['a.0'] = { libraryId: '1', trackId: '40' };
		getTracks.mockResolvedValue({ items: [] });

		const found = await lookup(['a.0', 'gone.0']);
		expect(found.get('a.0')).toBeNull();
		expect(found.get('gone.0')).toBeNull();
	});

	it('leaves out what could not be looked up, to be asked for again', async () => {
		refs['c.0'] = { libraryId: '1', trackId: '30' };
		getTracks.mockRejectedValueOnce(new Error('offline'));
		expect((await lookup(['c.0'])).has('c.0')).toBe(false);

		getTracks.mockResolvedValueOnce({ items: [summary('30')] });
		expect((await lookup(['c.0'])).get('c.0')?.title).toBe('Track 30');
	});
});
