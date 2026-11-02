// Where the queue panel's rows are. Every row of the panel has the same height, so what shows
// is worked out from the scroll position alone, and only the rows in view are read from the
// queue and drawn (`requirements/performance.md` §6). The Up Next tab is one run of rows: the
// Manual Queue's heading and items (or a hint when it is empty), then the context's heading
// and items. The items are one run of the worker's `upNext` section: the Manual Queue's first.

import type { QueueAnchor } from '$lib/api/types';

/** The height of every row, in pixels. */
export const ROW = 56;

/** The tallest a list is drawn, in pixels: Firefox drops a height past about 17.9M, so a longer one scrolls through a scaled range. */
export const MAX_SPACE = 10_000_000;

/** What one row of Up Next is; an item's `at` is its place in the `upNext` section. */
export type Row =
	| { kind: 'heading'; section: 'manual' | 'context' }
	| { kind: 'hint' }
	| { kind: 'item'; at: number };

/** A run of the `upNext` section's items, and the row of the first. */
export type Piece = { offset: number; limit: number; row: number };

/** The row of the context's heading: after the Manual Queue, or its hint when it is empty. */
const contextHeading = (manual: number) => 1 + Math.max(manual, 1);

/** How many rows Up Next has, from the Manual Queue's items and all of Up Next's. */
export const upNextRows = (manual: number, upNext: number) =>
	contextHeading(manual) + 1 + (upNext - manual);

/** What row `v` of Up Next is. */
export function upNextRow(v: number, manual: number): Row {
	const heading = contextHeading(manual);
	if (v === 0) return { kind: 'heading', section: 'manual' };
	if (v < heading) return manual > 0 ? { kind: 'item', at: v - 1 } : { kind: 'hint' };
	if (v === heading) return { kind: 'heading', section: 'context' };
	return { kind: 'item', at: manual + v - heading - 1 };
}

/** The runs of items to read for rows `first` up to `last` of Up Next. */
export function upNextPieces(first: number, last: number, manual: number, upNext: number) {
	const heading = contextHeading(manual);
	const pieces: Piece[] = [];
	const manualFrom = Math.max(first, 1);
	const manualTo = Math.min(last, 1 + manual);
	if (manualTo > manualFrom) {
		pieces.push({ offset: manualFrom - 1, limit: manualTo - manualFrom, row: manualFrom });
	}
	const from = Math.max(first, heading + 1);
	const to = Math.min(last, heading + 1 + (upNext - manual));
	if (to > from) {
		pieces.push({ offset: manual + from - heading - 1, limit: to - from, row: from });
	}
	return pieces;
}

/**
 * Where a drop at `gap`, the line before row `gap`, puts items: the Manual Queue's end just above
 * the context's heading and the context's first item just below it. `itemAt` names the item of a
 * row that is drawn, and a gap in front of one of the `moving` items is taken past it, since the
 * queue refuses an anchor it is moving.
 */
export function anchorAt(
	gap: number,
	manual: number,
	upNext: number,
	itemAt: (v: number) => string | undefined,
	moving: ReadonlySet<string>
): QueueAnchor {
	const rows = upNextRows(manual, upNext);
	const heading = contextHeading(manual);
	let g = gap;
	while (g < rows && moving.has(itemAt(g) ?? '')) g++;
	if (g <= 1) return { type: 'manualStart' };
	if (g <= heading) {
		const item = manual > 0 && g < heading ? itemAt(g) : undefined;
		return item ? { type: 'before', item } : { type: 'manualEnd' };
	}
	const item = g < rows ? itemAt(g) : undefined;
	return item ? { type: 'before', item } : { type: 'contextEnd' };
}
