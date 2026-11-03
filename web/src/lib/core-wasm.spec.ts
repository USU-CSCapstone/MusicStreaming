import { readFileSync } from 'node:fs';
import { expect, test } from 'vitest';
import { Queue, sortRows } from '$lib/core-wasm/jewelcase_core_wasm.js';

/**
 * The cases the native core's `permutation_golden_vectors` and `parity_edit_sequence` tests
 * check too (and `sort_golden_cases`), so the two cannot drift apart.
 */
const parity = JSON.parse(
	readFileSync(new URL('../../../crates/core/src/queue/parity.json', import.meta.url), 'utf8')
) as {
	permutation: [number, number, number, number][];
	started: {
		source: { key: string; libraryId: string; length: number };
		at: number;
		state: unknown;
	};
	initial: unknown;
	edits: unknown[];
	final: unknown;
	sort: { rows: unknown[]; cases: { field: string; order: string; positions: number[] }[] };
};

type Row = { item: string; source: string; position: number; track: string | null; play?: number };

/** A queue started on a source of `length` tracks at its first, as the server's state has it. */
function started(length: number, extra: object = {}) {
	return Queue.fromJson(
		JSON.stringify({
			sources: [{ key: 'a', libraryId: 'lib', list: 'h', length }],
			copies: [],
			manual: [],
			order: [{ view: { key: 'a' }, start: 0, len: length }],
			cursor: 1,
			current: 'a.0',
			history: [],
			nextPlay: 0,
			shuffle: null,
			repeat: 'off',
			adjusted: false,
			...extra
		})
	);
}

/** The body of `GET /me/queue/lists/{hash}` for these track IDs. */
const list = (...tracks: string[]) => JSON.stringify({ libraryId: 'lib', tracks });

test('permutation matches native vectors', () => {
	for (const [seed, m, x, y] of parity.permutation) {
		// Up Next row x of a shuffle over the m rows after the first reads base row π(x).
		const queue = started(m + 1);
		expect(queue.apply(JSON.stringify({ type: 'shuffle', on: true, seed }))).toBeUndefined();
		const [row] = JSON.parse(queue.upNextRows(x, 1)) as Row[];
		expect(row.position - 1, `seed ${seed} m ${m} x ${x}`).toBe(y);
		expect(queue.upNextLen()).toBe(m);
	}
});

test('sorting rows matches native', () => {
	const rows = JSON.stringify(parity.sort.rows);
	for (const { field, order, positions } of parity.sort.cases) {
		expect(Array.from(sortRows(rows, field, order)), `${field} ${order}`).toEqual(positions);
	}
	expect(() => sortRows(rows, 'bitrate', 'asc')).toThrow();
	expect(() => sortRows(rows, 'title', 'up')).toThrow();
	expect(() => sortRows('[{"id":"x"}]', 'title', 'asc')).toThrow();
});

test('a started queue matches native', () => {
	const { source, at, state } = parity.started;
	const queue = Queue.started(source.key, source.libraryId, source.length, at, 'off');
	expect(JSON.parse(queue.toJson())).toEqual(state);
	expect(() => Queue.started('a', 'lib', 3, 3, 'off')).toThrow();
	expect(() => Queue.started('a', 'lib', 3, 0, 'twice')).toThrow();
	// A start keeps the repeat mode (`requirements/queue.md` §5).
	expect(JSON.parse(Queue.started('a', 'lib', 3, 0, 'all').toJson()).repeat).toBe('all');
});

test('edits match native', () => {
	const queue = Queue.fromJson(JSON.stringify(parity.initial));
	for (const edit of parity.edits) {
		expect(queue.apply(JSON.stringify(edit))).toBeUndefined();
	}
	expect(JSON.parse(queue.toJson())).toEqual(parity.final);
});

test('rows fill when a list attaches', () => {
	const queue = started(3);
	expect(queue.upNextLen()).toBe(2);
	expect(JSON.parse(queue.upNextRows(0, 10))).toEqual([
		{ item: 'a.1', source: 'a', position: 1, track: null },
		{ item: 'a.2', source: 'a', position: 2, track: null }
	]);
	expect(JSON.parse(queue.listHashes())).toEqual(['h']);

	// A track ID past 2^53 stays exact as a string.
	expect(queue.attachList('h', list('1', '9007199254740993', '18446744073709551615'))).toBe(1);
	expect(JSON.parse(queue.upNextRows(0, 10))).toEqual([
		{ item: 'a.1', source: 'a', position: 1, track: '9007199254740993' },
		{ item: 'a.2', source: 'a', position: 2, track: '18446744073709551615' }
	]);
	expect(JSON.parse(queue.currentRow()!)).toMatchObject({ item: 'a.0', track: '1' });
	expect(queue.findItem('a.2')).toBe(1);
	expect(queue.findItem('a.0')).toBeUndefined();

	// A copy keeps the attached rows and edits apart from the original.
	const copy = queue.copy();
	copy.apply('{"type":"next"}');
	expect(JSON.parse(copy.upNextRows(0, 1))[0].track).toBe('18446744073709551615');
	expect(copy.historyLen()).toBe(1);
	expect(queue.historyLen()).toBe(0);
	expect(JSON.parse(queue.upNextRows(0, 1))[0].track).toBe('9007199254740993');
	expect(queue.attachList('other', list('1', '2', '3'))).toBe(0);
});

test('a queue adopts the lists another has attached, and only those', () => {
	const held = started(3);
	held.attachList('h', list('1', '2', '3'));
	const fresh = Queue.fromJson(held.toJson());
	expect(JSON.parse(fresh.upNextRows(0, 1))[0].track).toBeNull();

	expect(JSON.parse(fresh.adoptLists(held))).toEqual(['h']);
	expect(JSON.parse(fresh.upNextRows(0, 1))[0].track).toBe('2');
	expect(JSON.parse(fresh.adoptLists(started(3)))).toEqual([]);
});

test('a list for a source with no hash attaches by key', () => {
	const queue = Queue.fromJson(
		JSON.stringify({
			...JSON.parse(started(2).toJson()),
			sources: [{ key: 'a', libraryId: 'lib', length: 2 }]
		})
	);
	expect(JSON.parse(queue.listHashes())).toEqual([]);
	queue.attachSource('a', 'new', list('5', '6'));
	expect(JSON.parse(queue.listHashes())).toEqual(['new']);
	expect(JSON.parse(queue.upNextRows(0, 1))[0].track).toBe('6');
});

test('history rows run newest first with play numbers, and counts are numbers', () => {
	const queue = started(4);
	queue.attachList('h', list('10', '11', '12', '13'));
	queue.apply('{"type":"next"}');
	queue.apply('{"type":"next"}');
	expect(queue.historyLen()).toBe(2);
	expect(queue.manualLen()).toBe(0);
	expect(JSON.parse(queue.historyRows(0, 1))).toEqual([
		{ item: 'a.1', source: 'a', position: 1, track: '11', play: 1 }
	]);
	expect(JSON.parse(queue.historyRows(1, 5))).toEqual([
		{ item: 'a.0', source: 'a', position: 0, track: '10', play: 0 }
	]);
	expect(JSON.parse(queue.historyRows(2, 5))).toEqual([]);
});

test('a refused edit returns its code and changes nothing', () => {
	const queue = started(3);
	const before = queue.toJson();
	const edit = { type: 'move', items: ['a.1'], before: { type: 'before', item: 'a.99' } };
	expect(queue.apply(JSON.stringify(edit))).toBe('anchor_gone');
	const rewind = { type: 'rewind', play: { play: 9, item: 'a.0' }, key: 'k' };
	expect(queue.apply(JSON.stringify(rewind))).toBe('play_gone');
	expect(queue.toJson()).toBe(before);
	expect(() => queue.apply('{"type":"bogus"}')).toThrow();
	expect(() => queue.attachList('h', list('1'))).toThrow('length');
	expect(() => queue.attachList('h', list('x', '2', '3'))).toThrow();
});
