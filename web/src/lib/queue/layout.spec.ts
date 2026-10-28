import { describe, expect, it } from 'vitest';
import { anchorAt, upNextPieces, upNextRow, upNextRows } from './layout';

/** Items named after their rows, for a queue with `manual` Manual Queue items. */
const itemAt = (manual: number) => (v: number) => {
	const row = upNextRow(v, manual);
	return row.kind === 'item' ? `${row.list}${row.at}` : undefined;
};

describe('Up Next rows', () => {
	it('puts the Manual Queue under its heading, then the context under its own', () => {
		expect(upNextRows(2, 3)).toBe(7);
		expect([0, 1, 2, 3, 4, 5].map((v) => upNextRow(v, 2))).toEqual([
			{ kind: 'heading', section: 'manual' },
			{ kind: 'item', list: 'manual', at: 0 },
			{ kind: 'item', list: 'manual', at: 1 },
			{ kind: 'heading', section: 'context' },
			{ kind: 'item', list: 'upcoming', at: 0 },
			{ kind: 'item', list: 'upcoming', at: 1 }
		]);
	});

	it('shows a hint where the Manual Queue would be when it is empty', () => {
		expect(upNextRows(0, 1)).toBe(4);
		expect([1, 2, 3].map((v) => upNextRow(v, 0).kind)).toEqual(['hint', 'heading', 'item']);
	});

	it('reads only the items of the rows in view, however long the queue is', () => {
		const pieces = upNextPieces(250_000, 250_020, 3, 500_000);
		expect(pieces).toEqual([{ list: 'upcoming', offset: 249_995, limit: 20, row: 250_000 }]);
		expect(upNextPieces(0, 12, 3, 500_000)).toEqual([
			{ list: 'manual', offset: 0, limit: 3, row: 1 },
			{ list: 'upcoming', offset: 0, limit: 7, row: 5 }
		]);
		expect(upNextPieces(0, 3, 0, 5)).toEqual([]);
	});
});

describe('where a drop lands', () => {
	const at = (gap: number, manual = 2, upcoming = 3, moving: string[] = []) =>
		anchorAt(gap, manual, upcoming, itemAt(manual), new Set(moving));

	it('names the item it lands before, within either part', () => {
		expect(at(2)).toEqual({ type: 'before', item: 'manual1' });
		expect(at(5)).toEqual({ type: 'before', item: 'upcoming1' });
	});

	it('is the Manual Queue at its edges, and the context at its end', () => {
		expect(at(0)).toEqual({ type: 'manualStart' });
		expect(at(1)).toEqual({ type: 'manualStart' });
		expect(at(3)).toEqual({ type: 'manualEnd' });
		expect(at(8)).toEqual({ type: 'contextEnd' });
	});

	it('puts the gap under the context heading before its first item, and above it at the Manual end', () => {
		expect(at(4)).toEqual({ type: 'before', item: 'upcoming0' });
	});

	it('is the Manual Queue when it is empty and the drop is above the context heading', () => {
		expect(at(2, 0, 3)).toEqual({ type: 'manualEnd' });
		expect(at(3, 0, 3)).toEqual({ type: 'before', item: 'upcoming0' });
	});

	it('skips past items that are being moved, which the queue would refuse as an anchor', () => {
		expect(at(5, 2, 3, ['upcoming1'])).toEqual({ type: 'before', item: 'upcoming2' });
		expect(at(6, 2, 3, ['upcoming1', 'upcoming2'])).toEqual({ type: 'contextEnd' });
	});
});
