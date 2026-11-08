import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Page } from './api/types';

const listeners = new Set<(library: string) => void>();
vi.mock('./catalog/client.svelte', () => ({
	catalog: {
		onChanged: (listener: (library: string) => void) => {
			listeners.add(listener);
			return () => listeners.delete(listener);
		}
	}
}));

const { RELOAD_MS, Live } = await import('./live.svelte');
const { Paged } = await import('./paged.svelte');

/** Tells every listening page that `library` changed. */
const announce = (library = '1') => listeners.forEach((listener) => listener(library));

/**
 * A sorted list of `rows` served `size` at a time, with keyset cursors as the API has them: a
 * cursor is the last row of the page before. `delay.ms` makes each read take that long.
 */
function served(rows: () => string[], size = 2) {
	const calls: (string | undefined)[] = [];
	const delay = { ms: 0 };
	const load = async (cursor?: string): Promise<Page<string>> => {
		calls.push(cursor);
		const rest = rows().filter((row) => cursor === undefined || row > cursor);
		const items = rest.slice(0, size);
		const page = {
			items,
			nextCursor: rest.length > size ? items[items.length - 1] : null,
			total: rows().length
		};
		if (delay.ms) await new Promise((resolve) => setTimeout(resolve, delay.ms));
		return page;
	};
	return { calls, delay, load };
}

/** Shows `paged` as a page does, so it listens; returns what takes it off the page. */
function show(paged: { items?: unknown; value?: unknown }) {
	const destroy = $effect.root(() => {
		$effect(() => void (paged.items ?? paged.value));
	});
	flushSync();
	return async () => {
		destroy();
		await Promise.resolve();
	};
}

beforeEach(() => {
	vi.useFakeTimers();
});

afterEach(() => {
	vi.useRealTimers();
	expect(listeners.size).toBe(0);
});

describe('a library changing on the device', () => {
	it('re-reads the whole window loaded and swaps it in, continuing from its end', async () => {
		let rows = ['a', 'b', 'c', 'd', 'e', 'f'];
		const { calls, load } = served(() => rows);
		const paged = new Paged(await load(), load, '1');
		await paged.more();
		const stop = show(paged);
		calls.length = 0;

		rows = ['0', 'a', 'b', 'c', 'd', 'e', 'f'];
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);

		expect(calls).toEqual([undefined, 'a']);
		expect(paged.items).toEqual(['0', 'a', 'b', 'c']);
		expect(paged.total).toBe(7);
		expect(paged.hasMore).toBe(true);
		await paged.more();
		expect(paged.items).toEqual(['0', 'a', 'b', 'c', 'd', 'e']);
		await stop();
	});

	it('leaves the rows shown, by identity, when nothing in the window changed', async () => {
		let rows = ['a', 'b', 'c', 'd'];
		const { load } = served(() => rows);
		const paged = new Paged(await load(), load, '1');
		const stop = show(paged);
		const shown = paged.items;

		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(paged.items).toBe(shown);

		// A change past the window still updates the total.
		rows = [...rows, 'e'];
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(paged.items).toBe(shown);
		expect(paged.total).toBe(5);
		await stop();
	});

	it('does nothing for another library, and not before the burst is gathered', async () => {
		const { calls, load } = served(() => ['a', 'b', 'c']);
		const paged = new Paged(await load(), load, '1');
		const stop = show(paged);
		calls.length = 0;

		announce('2');
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 2);
		expect(calls).toEqual([]);

		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS - 1);
		expect(calls).toEqual([]);
		await vi.advanceTimersByTimeAsync(1);
		expect(calls).toEqual([undefined]);
		await stop();
	});

	it('reloads once for a burst', async () => {
		const { calls, load } = served(() => ['a', 'b']);
		const paged = new Paged(await load(), load, '1');
		const stop = show(paged);
		calls.length = 0;

		for (let i = 0; i < 20; i++) {
			announce();
			await vi.advanceTimersByTimeAsync(20);
		}
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 3);
		expect(calls).toHaveLength(1);
		await stop();
	});

	it('reloads once more, a throttle after, for what arrives during a reload', async () => {
		const { calls, delay, load } = served(() => ['a', 'b']);
		const paged = new Paged(await load(), load, '1');
		const stop = show(paged);
		calls.length = 0;
		delay.ms = 300;

		announce();
		// The reload began at RELOAD_MS and ends 300 ms later.
		await vi.advanceTimersByTimeAsync(RELOAD_MS + 100);
		expect(calls).toHaveLength(1);
		announce();
		// Not before a throttle has passed since the reload ended, and not concurrently.
		await vi.advanceTimersByTimeAsync(RELOAD_MS + 150);
		expect(calls).toHaveLength(1);
		await vi.advanceTimersByTimeAsync(100);
		expect(calls).toHaveLength(2);
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 5);
		expect(calls).toHaveLength(2);
		await stop();
	});

	it('does not reload for a page taken off screen, before its time or during a reload', async () => {
		const { calls, delay, load } = served(() => ['a', 'b']);
		const paged = new Paged(await load(), load, '1');
		let stop = show(paged);
		calls.length = 0;

		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS - 100);
		await stop();
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 5);
		expect(calls).toEqual([]);

		stop = show(paged);
		delay.ms = 300;
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS + 100);
		announce();
		await stop();
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 5);
		expect(calls).toHaveLength(1);
	});

	it('keeps what it shows when the reload fails, and reads again on the next change', async () => {
		let fail = true;
		let rows = ['a', 'b', 'c'];
		const { load } = served(() => rows);
		const flaky = async (cursor?: string) => {
			if (fail) throw new Error('offline');
			return load(cursor);
		};
		const paged = new Paged(await load(), flaky, '1');
		const stop = show(paged);

		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(paged.items).toEqual(['a', 'b']);

		fail = false;
		rows = ['a', 'b', 'c', 'd'];
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(paged.total).toBe(4);
		await stop();
	});

	it('is not listening while nothing displays it', async () => {
		const { load } = served(() => ['a']);
		const paged = new Paged(await load(), load, '1');
		expect(listeners.size).toBe(0);
		expect(paged.items).toEqual(['a']);
		expect(listeners.size).toBe(0);
	});
});

describe('loading more', () => {
	it('is one read at a time, so two asks do not repeat a page', async () => {
		const { delay, load } = served(() => ['a', 'b', 'c', 'd', 'e', 'f']);
		const paged = new Paged(await load(), load, '1');
		delay.ms = 100;
		const asks = Promise.all([paged.more(), paged.more()]);
		await vi.advanceTimersByTimeAsync(500);
		await asks;
		expect(paged.items).toEqual(['a', 'b', 'c', 'd']);
	});

	it('waits for a reload under way rather than running beside it', async () => {
		const { delay, load } = served(() => ['a', 'b', 'c', 'd', 'e', 'f']);
		const paged = new Paged(await load(), load, '1');
		const stop = show(paged);
		delay.ms = 300;
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS + 100);
		const asks = Promise.all([paged.more(), paged.more()]);
		await vi.advanceTimersByTimeAsync(1000);
		await asks;
		expect(paged.items).toEqual(['a', 'b']);
		const next = paged.more();
		await vi.advanceTimersByTimeAsync(1000);
		await next;
		expect(paged.items).toEqual(['a', 'b', 'c', 'd']);
		await stop();
	});

	it('is waited for by a reload that comes due meanwhile', async () => {
		let rows = ['a', 'b', 'c', 'd', 'e', 'f'];
		const { delay, load } = served(() => rows);
		const paged = new Paged(await load(), load, '1');
		const stop = show(paged);
		delay.ms = 2000;
		const more = paged.more();
		rows = ['0', ...rows];
		announce();
		// The slow read is under way; the reload would be quick.
		delay.ms = 0;
		await vi.advanceTimersByTimeAsync(5000);
		await more;
		expect(paged.items).toEqual(['0', 'a', 'b', 'c']);
		await stop();
	});
});

describe('a single read that reads again', () => {
	it('shows the new read, and keeps the old one by identity when it is the same', async () => {
		const first = { name: 'a' };
		let now = { name: 'a' };
		const live = new Live(first, '1', async () => now);
		const stop = show(live);

		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(live.value).toBe(first);

		now = { name: 'b' };
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(live.value).toEqual({ name: 'b' });
		await stop();
	});
});
