import { describe, expect, it } from 'vitest';
import type { Playable, PlaylistOp } from './api/types';
import { applyOps, invertOps } from './playlist-ops';

// Items are track IDs. An album expands to two tracks, `<id>.1` and `<id>.2`.
const resolve = (items: Playable[]) =>
	items.flatMap((p) => {
		if (p.type === 'external') return [p.externalId];
		if (p.type === 'album') return [`${p.id}.1`, `${p.id}.2`];
		if (p.type === 'playlist') return []; // An empty one.
		return [p.id];
	});
const track = (id: string): Playable => ({ type: 'track', id });
const apply = (items: string[], ops: PlaylistOp[]) => applyOps(items, ops, resolve);
const invert = (items: string[], ops: PlaylistOp[]) => invertOps(items, ops, resolve, track).ops;
/** `items` after `ops`, then after their undo. */
const roundTrip = (items: string[], ops: PlaylistOp[]) => {
	const undo = invertOps(items, ops, resolve, track);
	return applyOps(apply(items, ops), undo.ops, undo.resolve);
};

const abc = ['a', 'b', 'c'];
const twelve = 'abcdefghijkl'.split('');

describe('add', () => {
	it('appends without a position', () => {
		expect(apply(abc, [{ op: 'add', items: [track('x'), track('y')] }])).toEqual([
			'a',
			'b',
			'c',
			'x',
			'y'
		]);
	});

	it('puts the first added track at the position', () => {
		expect(apply(abc, [{ op: 'add', items: [track('x'), track('y')], at: 0 }])).toEqual([
			'x',
			'y',
			'a',
			'b',
			'c'
		]);
		expect(apply(abc, [{ op: 'add', items: [track('x')], at: 3 }])).toEqual(['a', 'b', 'c', 'x']);
	});

	it('expands albums to their tracks in order', () => {
		expect(apply(['a'], [{ op: 'add', items: [{ type: 'album', id: 'z' }], at: 0 }])).toEqual([
			'z.1',
			'z.2',
			'a'
		]);
	});

	it('adds external tracks', () => {
		const ext: Playable = { type: 'external', sourceId: 's', externalId: 'e' };
		expect(apply(['a'], [{ op: 'add', items: [ext] }])).toEqual(['a', 'e']);
	});

	it('allows a track the playlist already has', () => {
		expect(apply(abc, [{ op: 'add', items: [track('a')] }])).toEqual(['a', 'b', 'c', 'a']);
	});

	it('refuses a position past the end, and no items', () => {
		expect(() => apply(abc, [{ op: 'add', items: [track('x')], at: 4 }])).toThrow(RangeError);
		expect(() => apply(abc, [{ op: 'add', items: [] }])).toThrow(RangeError);
	});
});

describe('remove', () => {
	it('removes positions that are not adjacent', () => {
		expect(apply(twelve, [{ op: 'remove', positions: [1, 6, 11] }]).join('')).toBe('acdefhijk');
	});

	it('removes one copy of a track held twice', () => {
		expect(apply(['a', 'b', 'a'], [{ op: 'remove', positions: [2] }])).toEqual(['a', 'b']);
	});

	it('refuses positions out of range, given twice, or none', () => {
		for (const positions of [[3], [-1], [0.5], [1, 1], []]) {
			expect(() => apply(abc, [{ op: 'remove', positions }]), String(positions)).toThrow(
				RangeError
			);
		}
	});
});

describe('move', () => {
	it('moves a scattered selection together, keeping its order', () => {
		// The 2nd, 7th, and 12th, as in `requirements/playlists.md` §4.
		expect(apply(twelve, [{ op: 'move', positions: [1, 6, 11], at: 0 }]).join('')).toBe(
			'bglacdefhijk'
		);
		expect(apply(twelve, [{ op: 'move', positions: [1, 6, 11], at: 4 }]).join('')).toBe(
			'acdebglfhijk'
		);
	});

	it('keeps playlist order however the positions are listed', () => {
		expect(apply(twelve, [{ op: 'move', positions: [11, 1, 6], at: 0 }]).join('')).toBe(
			'bglacdefhijk'
		);
	});

	it('places the first moved track at the position once the move is done', () => {
		expect(apply(abc, [{ op: 'move', positions: [0], at: 2 }])).toEqual(['b', 'c', 'a']);
		expect(apply(abc, [{ op: 'move', positions: [0, 1], at: 1 }])).toEqual(['c', 'a', 'b']);
		expect(apply(abc, [{ op: 'move', positions: [2], at: 0 }])).toEqual(['c', 'a', 'b']);
	});

	it('refuses a position the moved tracks would not fit at', () => {
		expect(() => apply(abc, [{ op: 'move', positions: [0, 1], at: 2 }])).toThrow(RangeError);
		expect(() => apply(abc, [{ op: 'move', positions: [3], at: 0 }])).toThrow(RangeError);
	});
});

describe('applyOps', () => {
	it('applies each op to the playlist as the ones before it left it', () => {
		const ops: PlaylistOp[] = [
			{ op: 'remove', positions: [0] },
			{ op: 'add', items: [track('x')], at: 0 },
			{ op: 'move', positions: [0], at: 2 }
		];
		expect(apply(abc, ops)).toEqual(['b', 'c', 'x']);
	});

	it('applies nothing when a later op is out of range', () => {
		const items = [...abc];
		const ops: PlaylistOp[] = [
			{ op: 'remove', positions: [0] },
			{ op: 'remove', positions: [2] }
		];
		expect(() => apply(items, ops)).toThrow(RangeError);
		expect(items).toEqual(abc);
	});

	it('leaves the playlist given to it alone', () => {
		const items = [...abc];
		apply(items, [{ op: 'move', positions: [0], at: 2 }]);
		expect(items).toEqual(abc);
	});
});

describe('invertOps', () => {
	const undoes = (items: string[], ops: PlaylistOp[]) =>
		expect(roundTrip(items, ops)).toEqual(items);

	it('removes what an add added', () => {
		const ops: PlaylistOp[] = [{ op: 'add', items: [{ type: 'album', id: 'z' }], at: 1 }];
		expect(invert(abc, ops)).toEqual([{ op: 'remove', positions: [1, 2] }]);
		undoes(abc, ops);
	});

	it('undoes nothing for an add that expanded to nothing', () => {
		expect(invert(abc, [{ op: 'add', items: [{ type: 'playlist', id: 'p' }] }])).toEqual([]);
	});

	it('adds removed tracks back at their positions, one add per run', () => {
		const ops: PlaylistOp[] = [{ op: 'remove', positions: [7, 1, 2, 11] }];
		expect(invert(twelve, ops)).toEqual([
			{ op: 'add', at: 1, items: [track('b'), track('c')] },
			{ op: 'add', at: 7, items: [track('h')] },
			{ op: 'add', at: 11, items: [track('l')] }
		]);
		undoes(twelve, ops);
	});

	it('moves a scattered selection back a run at a time, leaving one already in place', () => {
		const ops: PlaylistOp[] = [{ op: 'move', positions: [1, 2, 5, 9, 10], at: 3 }];
		expect(invert(twelve, ops)).toEqual([
			{ op: 'move', positions: [3, 4], at: 1 },
			{ op: 'move', positions: [6, 7], at: 9 }
		]);
		undoes(twelve, ops);
	});

	it('moves a run back in one move', () => {
		expect(invert(twelve, [{ op: 'move', positions: [2, 3, 4], at: 6 }])).toEqual([
			{ op: 'move', positions: [6, 7, 8], at: 2 }
		]);
	});

	it('undoes a move that changed nothing with nothing', () => {
		expect(invert(abc, [{ op: 'move', positions: [1], at: 1 }])).toEqual([]);
	});

	it('undoes several ops in one edit, last first', () => {
		undoes(abc, [
			{ op: 'remove', positions: [0] },
			{ op: 'add', items: [track('x'), track('y')], at: 0 },
			{ op: 'move', positions: [0, 3], at: 1 }
		]);
	});

	it('tells apart copies of a track held twice', () => {
		const items = ['a', 'b', 'a', 'c', 'a'];
		undoes(items, [{ op: 'move', positions: [0, 4], at: 1 }]);
		undoes(items, [{ op: 'remove', positions: [2] }]);
	});

	it('brings back the very items removed, not others like them', () => {
		const items = [{ id: 'a' }, { id: 'b' }, { id: 'a' }];
		const ops: PlaylistOp[] = [{ op: 'remove', positions: [1, 2] }];
		const undo = invertOps(
			items,
			ops,
			() => [],
			(item) => track(item.id)
		);
		const back = applyOps(
			applyOps(items, ops, () => []),
			undo.ops,
			undo.resolve
		);
		expect(back).toHaveLength(3);
		back.forEach((item, i) => expect(item).toBe(items[i]));
	});

	it('undoes any edit exactly', () => {
		// Seeded, so a failure always reproduces.
		let seed = 1;
		const random = (n: number) => {
			seed = (seed * 48271) % 2147483647;
			return seed % n;
		};
		const pick = (length: number) => {
			const all = Array.from({ length }, (_, i) => i).filter(() => random(3) === 0);
			return all.length ? all : [random(length)];
		};

		for (let run = 0; run < 500; run++) {
			// Few distinct tracks, so the playlist is full of duplicates.
			let items = Array.from({ length: random(15) }, () => 'abcde'[random(5)]);
			const original = items;
			const ops: PlaylistOp[] = [];
			for (let n = random(4) + 1; n > 0; n--) {
				let op: PlaylistOp;
				const kind = items.length ? random(3) : 0;
				if (kind === 0) {
					const added = Array.from({ length: random(3) + 1 }, () => track('xyz'[random(3)]));
					op = { op: 'add', items: added, at: random(items.length + 1) };
				} else if (kind === 1) {
					op = { op: 'remove', positions: pick(items.length) };
				} else {
					const positions = pick(items.length);
					op = { op: 'move', positions, at: random(items.length - positions.length + 1) };
				}
				ops.push(op);
				items = apply(items, [op]);
			}
			expect(roundTrip(original, ops), JSON.stringify({ original, ops })).toEqual(original);
		}
	});
});
