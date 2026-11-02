import { describe, expect, it } from 'vitest';
import { anchorAt, upNextPieces, upNextRow, upNextRows } from './layout';

/** Items named after their places in Up Next, for a queue with `manual` Manual Queue items. */
const itemAt = (manual: number) => (v: number) => {
	const row = upNextRow(v, manual);
	return row.kind === 'item' ? `item${row.at}` : undefined;
};

describe('Up Next rows', () => {
	it('puts the Manual Queue under its heading, then the context under its own', () => {
		// Two Manual items, then three of the context.
		expect(upNextRows(2, 5)).toBe(7);
		expect([0, 1, 2, 3, 4, 5, 6].map((v) => upNextRow(v, 2))).toEqual([
			{ kind: 'heading', section: 'manual' },
			{ kind: 'item', at: 0 },
			{ kind: 'item', at: 1 },
			{ kind: 'heading', section: 'context' },
			{ kind: 'item', at: 2 },
			{ kind: 'item', at: 3 },
			{ kind: 'item', at: 4 }
		]);
	});

	it('shows a hint where the Manual Queue would be when it is empty', () => {
		expect(upNextRows(0, 1)).toBe(4);
		expect([1, 2, 3].map((v) => upNextRow(v, 0))).toEqual([
			{ kind: 'hint' },
			{ kind: 'heading', section: 'context' },
			{ kind: 'item', at: 0 }
		]);
	});

	it('windows the items to read to the rows in view, however long the queue is', () => {
		// Row 250,000 is the context's item 249,995 of 500,000, after three Manual items.
		expect(upNextPieces(250_000, 250_020, 3, 500_003)).toEqual([
			{ offset: 3 + 249_995, limit: 20, row: 250_000 }
		]);
		expect(upNextPieces(0, 12, 3, 500_003)).toEqual([
			{ offset: 0, limit: 3, row: 1 },
			{ offset: 3, limit: 7, row: 5 }
		]);
		expect(upNextPieces(0, 3, 0, 0)).toEqual([]);
	});

	it('reads the same items as the rows it draws', () => {
		for (const [manual, upNext] of [
			[0, 4],
			[2, 5],
			[3, 3]
		]) {
			const rows = upNextRows(manual, upNext);
			const read = upNextPieces(0, rows, manual, upNext).flatMap((p) =>
				Array.from({ length: p.limit }, (_, k) => [p.row + k, p.offset + k])
			);
			const drawn = Array.from({ length: rows }, (_, v) => ({
				v,
				row: upNextRow(v, manual)
			})).flatMap(({ v, row }) => (row.kind === 'item' ? [[v, row.at]] : []));
			expect(read).toEqual(drawn);
		}
	});
});

describe('where a drop lands', () => {
	/** Where a drop at `gap` lands, with defaults for the queue's shape: 2 Manual, 3 of the context. */
	const at = (gap: number, manual = 2, upNext = 5, moving: string[] = []) =>
		anchorAt(gap, manual, upNext, itemAt(manual), new Set(moving));

	it('names the item it lands before, within either part', () => {
		expect(at(2)).toEqual({ type: 'before', item: 'item1' });
		expect(at(5)).toEqual({ type: 'before', item: 'item3' });
	});

	it('is the Manual Queue at its edges, and the context at its end', () => {
		expect(at(0)).toEqual({ type: 'manualStart' });
		expect(at(1)).toEqual({ type: 'manualStart' });
		expect(at(3)).toEqual({ type: 'manualEnd' });
		expect(at(8)).toEqual({ type: 'contextEnd' });
	});

	it('puts the gap under the context heading before its first item, and above it at the Manual end', () => {
		expect(at(4)).toEqual({ type: 'before', item: 'item2' });
	});

	it('is the Manual Queue when it is empty and the drop is above the context heading', () => {
		expect(at(2, 0, 3)).toEqual({ type: 'manualEnd' });
		expect(at(3, 0, 3)).toEqual({ type: 'before', item: 'item0' });
	});

	it('skips past items that are being moved, which the queue would refuse as an anchor', () => {
		expect(at(5, 2, 5, ['item3'])).toEqual({ type: 'before', item: 'item4' });
		expect(at(6, 2, 5, ['item3', 'item4'])).toEqual({ type: 'contextEnd' });
	});
});
