// Playlist edits applied on the device, so an edit shows at once and can be undone
// (`requirements/playlists.md` §4). An op means here exactly what it means to `editPlaylist`
// (`api/openapi.yaml`, `PlaylistOp`), so the device and the server reach the same order.
//
// Ordering belongs to the shared core (`design/general.md` §3), which the web client does not
// have yet. When it does, this module is replaced by calls to it.

// With its extension: the mock loads this module into the Vite config, which needs one.
import type { Playable, PlaylistOp } from './api/types.ts';

/** An add's items as playlist items: albums, artists, and playlists expand to their tracks in order. */
export type Resolve<T> = (items: Playable[]) => T[];

/**
 * `items` after `ops`, each applied to the playlist as the ones before it left it. A
 * position out of range, or a malformed op, throws a `RangeError` and nothing applies.
 */
export function applyOps<T>(
	items: readonly T[],
	ops: readonly PlaylistOp[],
	resolve: Resolve<T>
): T[] {
	let list = [...items];
	for (const op of ops) list = applyOp(list, op, resolve);
	return list;
}

/**
 * The ops that turn the playlist `ops` make from `items` back into `items`, sent as one
 * further edit (`api/openapi.yaml`, `editPlaylist`). A removal comes back as adds at the old
 * positions, named by `toPlayable`; a move comes back as single-item moves.
 */
export function invertOps<T>(
	items: readonly T[],
	ops: readonly PlaylistOp[],
	resolve: Resolve<T>,
	toPlayable: (item: T) => Playable
): PlaylistOp[] {
	// Slots give every item an identity, so a track held twice is still two items.
	type Slot = { item: T };
	let list: Slot[] = items.map((item) => ({ item }));
	const undo: PlaylistOp[][] = [];
	for (const op of ops) {
		const next = applyOp(list, op, (added) => resolve(added).map((item) => ({ item })));
		undo.push(invertOp(list, next, op, (slot) => toPlayable(slot.item)));
		list = next;
	}
	return undo.reverse().flat();
}

function applyOp<T>(list: readonly T[], op: PlaylistOp, resolve: Resolve<T>): T[] {
	switch (op.op) {
		case 'add': {
			if (op.items.length === 0) throw new RangeError('add has no items');
			const at = op.at ?? list.length;
			check(at, list.length);
			return list.toSpliced(at, 0, ...resolve(op.items));
		}
		case 'remove': {
			const drop = new Set(selection(op.positions, list.length));
			return list.filter((_, i) => !drop.has(i));
		}
		case 'move': {
			const moving = selection(op.positions, list.length);
			const skip = new Set(moving);
			const rest = list.filter((_, i) => !skip.has(i));
			check(op.at, rest.length);
			return rest.toSpliced(op.at, 0, ...moving.map((i) => list[i]));
		}
	}
}

/** The ops that turn `after` back into `before`, where `op` made one from the other. */
function invertOp<T>(
	before: readonly T[],
	after: readonly T[],
	op: PlaylistOp,
	toPlayable: (item: T) => Playable
): PlaylistOp[] {
	switch (op.op) {
		case 'add': {
			// An album or playlist may expand to nothing.
			const at = op.at ?? before.length;
			const count = after.length - before.length;
			return count ? [{ op: 'remove', positions: range(at, at + count) }] : [];
		}
		case 'remove': {
			// One add per contiguous run, in playlist order: everything before a run is back in
			// place by the time it is added, so its old position is right again.
			const ops: PlaylistOp[] = [];
			for (const run of runs(selection(op.positions, before.length))) {
				ops.push({ op: 'add', at: run[0], items: run.map((i) => toPlayable(before[i])) });
			}
			return ops;
		}
		case 'move': {
			// In playlist order, put each moved item straight after the one it followed before.
			// That one is either unmoved or already back, and nothing later lands between them.
			const ops: PlaylistOp[] = [];
			let list = [...after];
			for (const p of selection(op.positions, before.length)) {
				const item = before[p];
				const from = list.indexOf(item);
				const rest = list.toSpliced(from, 1);
				const to = p === 0 ? 0 : rest.indexOf(before[p - 1]) + 1;
				if (from !== to) ops.push({ op: 'move', positions: [from], at: to });
				list = rest.toSpliced(to, 0, item);
			}
			return ops;
		}
	}
}

/** A remove's or move's positions in playlist order, which is the order a move keeps. */
function selection(positions: readonly number[], length: number): number[] {
	if (positions.length === 0) throw new RangeError('no positions');
	const sorted = positions.toSorted((a, b) => a - b);
	sorted.forEach((p, i) => {
		check(p, length - 1);
		if (p === sorted[i - 1]) throw new RangeError(`position ${p} is given twice`);
	});
	return sorted;
}

/** Throws unless `p` is a whole position from 0 to `max`. */
function check(p: number, max: number) {
	if (!Number.isInteger(p) || p < 0 || p > max) {
		throw new RangeError(`position ${p} is out of range`);
	}
}

const range = (start: number, end: number) =>
	Array.from({ length: end - start }, (_, i) => start + i);

/** Ascending positions split into runs of consecutive ones. */
function runs(sorted: number[]): number[][] {
	const out: number[][] = [];
	for (const p of sorted) {
		const last = out.at(-1);
		if (last && last.at(-1) === p - 1) last.push(p);
		else out.push([p]);
	}
	return out;
}
