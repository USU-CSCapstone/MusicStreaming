import { afterEach, describe, expect, it, vi } from 'vitest';
import type { TrackSummary } from '$lib/api/types';
import { lookup, trackKey } from './summaries';

/** A track with the given ID, as the lookup returns it. */
const track = (id: string) => ({ id, title: `Track ${id}` }) as TrackSummary;

/** Answers `ids` lookups with the tracks of `known`, recording each request's URL. */
function serve(known: Set<string>, requests: URL[] = []) {
	vi.stubGlobal(
		'fetch',
		vi.fn(async (url: string) => {
			const u = new URL(url, 'http://x');
			requests.push(u);
			const ids = (u.searchParams.get('ids') ?? '').split(',').filter((id) => known.has(id));
			return Response.json({ items: ids.map(track), total: ids.length, nextCursor: null });
		})
	);
	return requests;
}

afterEach(() => vi.unstubAllGlobals());

describe('lookup', () => {
	it('asks once per library, a hundred IDs at a time, and names each track once', async () => {
		const known = new Set(Array.from({ length: 250 }, (_, i) => String(i)));
		const requests = serve(known);
		const refs = Array.from({ length: 250 }, (_, i) => ({ libraryId: '11', trackId: String(i) }));
		const found = await lookup([...refs, refs[0], { libraryId: '12', trackId: '3' }]);
		expect(requests.map((u) => u.pathname.split('/').at(-2))).toEqual(['11', '11', '11', '12']);
		expect(requests.slice(0, 3).map((u) => u.searchParams.get('ids')!.split(',').length)).toEqual([
			100, 100, 50
		]);
		expect(found.get(trackKey('11', '7'))?.title).toBe('Track 7');
		expect(found.get(trackKey('12', '3'))?.title).toBe('Track 3');
	});

	it('maps a track the library did not give to null, and never asks for none', async () => {
		const requests = serve(new Set(['1']));
		expect((await lookup([])).size).toBe(0);
		const found = await lookup([
			{ libraryId: '11', trackId: '1' },
			{ libraryId: '11', trackId: '2' }
		]);
		expect(found.get(trackKey('11', '2'))).toBeNull();
		expect(found.get(trackKey('11', '1'))).not.toBeNull();
		expect(requests).toHaveLength(1);
	});

	it('leaves out what a failed request would have given, so it is asked again', async () => {
		vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('offline')));
		expect((await lookup([{ libraryId: '11', trackId: '1' }])).size).toBe(0);
	});
});
