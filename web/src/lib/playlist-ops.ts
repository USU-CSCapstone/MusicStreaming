// Playlist edits applied on the device, so an edit shows at once and can be undone
// (`requirements/playlists.md` §4). An op means here exactly what it means to `editPlaylist`
// (`api/openapi.yaml`, `PlaylistOp`), so the device and the server reach the same order.
//
// Ordering belongs to the shared core (`design/general.md` §3), which the web client does not
// have yet. When it does, this module is replaced by calls to it.

// With its extension: the mock loads this module into the Vite config, which needs one.
import type { Playable, PlaylistOp } from './api/types.ts';

/**
 * An add's items as playlist items: albums, artists, and playlists expand to their tracks in
 * order. It is given the add's own `items`, never a copy.
 */
export type Resolve<T> = (items: Playable[]) => T[];

/** An edit's undo: the ops that reverse it, and the items its adds bring back. */
export type Inverse<T> = { ops: PlaylistOp[]; resolve: Resolve<T> };

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
 * positions, named by `toPlayable`; a move comes back as a move per run of the items moved.
 * Applied with the `resolve` returned alongside them, those adds bring back the very items
 * removed.
 */
export function invertOps<T>(
	items: readonly T[],
	ops: readonly PlaylistOp[],
	resolve: Resolve<T>,
	toPlayable: (item: T) => Playable
): Inverse<T> {
	let list = [...items];
	// Each add's items, by the array it names them with, so nothing is looked up.
	const removed = new Map<readonly Playable[], T[]>();
	const restore = (back: T[]) => {
		const named = back.map(toPlayable);
		removed.set(named, back);
		return named;
	};
	const undo: PlaylistOp[][] = [];
	for (const op of ops) {
		const next = applyOp(list, op, resolve);
		undo.push(invertOp(list, next, op, restore));
		list = next;
	}
	return {
		ops: undo.reverse().flat(),
		resolve: (added) => {
			const back = removed.get(added);
			if (!back) throw new Error('Undo brings back only what was removed.');
			return back;
		}
	};
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
	restore: (removed: T[]) => Playable[]
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
				ops.push({ op: 'add', at: run[0], items: restore(run.map((i) => before[i])) });
			}
			return ops;
		}
		case 'move': {
			// The moved items sit together from `op.at` in playlist order, so a run of them is still
			// a run. Runs going left go back in playlist order, then runs going right last first.
			// Either way the runs before a run are just ahead of it, and only what preceded it lies
			// ahead of its old start: it moves from `op.at` plus their length straight back there.
			const left: PlaylistOp[] = [];
			const right: PlaylistOp[] = [];
			let ahead = 0;
			for (const run of runs(selection(op.positions, before.length))) {
				const from = op.at + ahead;
				const back: PlaylistOp = {
					op: 'move',
					positions: range(from, from + run.length),
					at: run[0]
				};
				if (run[0] < from) left.push(back);
				else if (run[0] > from) right.push(back);
				ahead += run.length;
			}
			return [...left, ...right.reverse()];
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
