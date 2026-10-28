import { readFileSync } from 'node:fs';
import { expect, test } from 'vitest';
import { initSync, Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';

initSync({
	module: readFileSync(new URL('./core-wasm/jewelcase_core_wasm_bg.wasm', import.meta.url))
});

/** An 11-track source `a` in library `lib`, as the wire carries it. */
const source = JSON.stringify({
	key: 'a',
	libraryId: 'lib',
	tracks: Array.from({ length: 11 }, (_, i) => `a-${i}`)
});

/** Queue item IDs for `a` at these positions in its source. */
const ids = (...positions: number[]) => positions.map((i) => `a.${i}`);

test('shuffling with seed 42 gives the order the native core pins', () => {
	const queue = Queue.start(source, 0);
	queue.apply(JSON.stringify({ type: 'shuffle', on: true, seed: 42 }));

	// The same permutation as `shuffle_golden_queue_order_follows_the_pinned_permutation`.
	expect(queue.current()).toBe('a.0');
	expect(queue.upcoming(0, 100)).toEqual(ids(1, 10, 6, 9, 7, 5, 8, 3, 2, 4));
	expect(queue.upcoming(2, 3)).toEqual(ids(6, 9, 7));
});

test('edits, lookups and snapshots work and round-trip', () => {
	const queue = Queue.start(source, 0);
	const added = { key: 'm', libraryId: 'other', tracks: ['m-0'] };
	queue.apply(JSON.stringify({ type: 'add', source: added, before: { type: 'manualEnd' } }));
	queue.apply(JSON.stringify({ type: 'next' }));

	expect(queue.current()).toBe('m.0');
	expect(queue.history(0, 10)).toEqual(ids(0));
	expect(queue.manual()).toEqual([]);
	expect(queue.track('m.0')).toEqual(['other', 'm-0']);
	expect(queue.track('zzz.1')).toBeUndefined();

	const restored = Queue.fromSnapshot(queue.snapshot());
	expect(restored.snapshot()).toBe(queue.snapshot());
	expect(restored.upcoming(0, 100)).toEqual(ids(1, 2, 3, 4, 5, 6, 7, 8, 9, 10));
});

test('a refused edit throws the core error and changes nothing', () => {
	const queue = Queue.start(source, 0);
	const before = queue.snapshot();
	const edit = { type: 'move', items: ids(1), before: { type: 'before', item: 'a.99' } };

	expect(() => queue.apply(JSON.stringify(edit))).toThrow('no longer in Up Next');
	expect(queue.snapshot()).toBe(before);
	expect(() => Queue.start(source, 11)).toThrow('past the end');
	expect(() => queue.apply('{"type":"bogus"}')).toThrow();
});
