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
		expect([...first.values()].map((t) => t?.track.title)).toEqual([
			'Track 10',
			'Track 11',
			'Track 20'
		]);
		expect(getTracks.mock.calls.map(([, library, ids]) => [library, ids])).toEqual([
			['1', ['10', '11']],
			['2', ['20']]
		]);

		getTracks.mockClear();
		await lookup(['a.0']);
		expect(getTracks).not.toHaveBeenCalled();
	});

	it('keeps the same track ID in two libraries apart', async () => {
		Object.assign(refs, {
			'a.0': { libraryId: '1', trackId: '60' },
			'b.0': { libraryId: '2', trackId: '60' }
		});
		getTracks.mockImplementation(async (_f, library: string, ids: string[]) => ({
			items: ids.map((id) => ({ ...summary(id), title: `Track ${id} of ${library}` }))
		}));

		const found = await lookup(['a.0', 'b.0']);
		expect(found.get('a.0')?.track.title).toBe('Track 60 of 1');
		expect(found.get('b.0')?.track.title).toBe('Track 60 of 2');
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
		expect((await lookup(['c.0'])).get('c.0')?.track.title).toBe('Track 30');
	});

	it('maps the items of a library that answers nothing to null, and still finds the others', async () => {
		Object.assign(refs, {
			'gone.0': { libraryId: '1', trackId: '50' },
			'gone.1': { libraryId: '1', trackId: '51' },
			'here.0': { libraryId: '2', trackId: '65' }
		});
		// A library out of reach answers 404, which `getTracks` turns into an empty page.
		getTracks.mockImplementation(async (_f, library: string, ids: string[]) => ({
			items: library === '1' ? [] : ids.map(summary)
		}));

		const found = await lookup(['gone.0', 'gone.1', 'here.0']);
		expect(found.get('gone.0')).toBeNull();
		expect(found.get('gone.1')).toBeNull();
		expect(found.get('here.0')?.track.title).toBe('Track 65');
	});

	it('asks for at most 100 tracks at once, as the server takes', async () => {
		const items = Array.from({ length: 150 }, (_, i) => `many.${i}`);
		items.forEach((item, i) => (refs[item] = { libraryId: '1', trackId: `m${i}` }));
		getTracks.mockImplementation(async (_f, _library: string, ids: string[]) => {
			if (ids.length > 100) throw new Error('422');
			return { items: ids.map(summary) };
		});

		const found = await lookup(items);
		expect(getTracks.mock.calls.map(([, , ids]) => ids.length)).toEqual([100, 50]);
		expect(found.size).toBe(150);
	});
});
