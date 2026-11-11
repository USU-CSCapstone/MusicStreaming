import { flushSync } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Page } from './api/types';
import { ALL, type Kind } from './catalog/kinds';

const listeners = new Set<(library: string, kinds: readonly Kind[]) => void>();
vi.mock('./catalog/client.svelte', async () => {
	const { Changes } = await import('./catalog/changes');
	const changes = new Changes();
	return {
		catalog: {
			onChanged: (listener: (library: string, kinds: readonly Kind[]) => void) => {
				listeners.add(listener);
				return () => listeners.delete(listener);
			},
			get changes() {
				return changes.count;
			},
			changedSince: (count: number, library: string, kinds: readonly Kind[]) =>
				changes.since(count, library, kinds),
			/** What the client does on hearing an announcement, whoever listens. */
			record: (library: string, kinds: readonly Kind[]) => changes.record(library, kinds)
		}
	};
});
const { catalog } = (await import('./catalog/client.svelte')) as unknown as {
	catalog: { changes: number; record: (library: string, kinds: readonly Kind[]) => void };
};

const { DEPENDS, RELOAD_MS, Live } = await import('./live.svelte');
const { Paged } = await import('./paged.svelte');

/** Tells every listening page that `library` changed in `kinds`, every kind unless said. */
const announce = (library = '1', kinds: readonly Kind[] = ALL) => {
	catalog.record(library, kinds);
	listeners.forEach((listener) => listener(library, kinds));
};

/**
 * A sorted list of `rows` served `size` at a time, with keyset cursors as the API has them: a
 * cursor is the last row of the page before. `delay.ms` makes each read take that long.
 */
function served(rows: () => string[], size = 2) {
	const calls: (string | undefined)[] = [];
	const limits: (number | undefined)[] = [];
	const delay = { ms: 0 };
	/** Reads the page after `cursor`, at most `limit` rows, recording both and waiting `delay.ms`. */
	const load = async (cursor?: string, limit?: number): Promise<Page<string>> => {
		calls.push(cursor);
		limits.push(limit);
		const rest = rows().filter((row) => cursor === undefined || row > cursor);
		// Like the API, a limit caps the page.
		const take = limit ?? size;
		const items = rest.slice(0, take);
		const page = {
			items,
			nextCursor: rest.length > take ? items[items.length - 1] : null,
			total: rows().length
		};
		if (delay.ms) await new Promise((resolve) => setTimeout(resolve, delay.ms));
		return page;
	};
	return { calls, limits, delay, load };
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
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		await paged.more();
		const stop = show(paged);
		calls.length = 0;

		rows = ['0', 'a', 'b', 'c', 'd', 'e', 'f'];
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);

		expect(calls).toEqual([undefined]);
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
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
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
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
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

	it('re-reads a long window in pages of at most 1000, while more() keeps its own size', async () => {
		const rows = Array.from({ length: 2500 }, (_, i) => String(i).padStart(4, '0'));
		const { limits, load } = served(() => rows, 1200);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		const stop = show(paged);
		await paged.more();
		expect(limits).toEqual([undefined, undefined]);
		limits.length = 0;

		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 2);
		expect(limits).toEqual([1000, 1000, 400]);
		expect(paged.items).toHaveLength(2400);
		await stop();
	});

	it('reads back no more rows than are shown, so an unchanged window stays as it is', async () => {
		const rows = Array.from({ length: 30 }, (_, i) => String(i).padStart(2, '0'));
		const { limits, load } = served(() => rows);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		const stop = show(paged);
		const shown = paged.items;
		limits.length = 0;

		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 2);
		expect(paged.items).toBe(shown);
		expect(paged.items).toHaveLength(2);
		expect(limits).toEqual([2]);
		await stop();
	});

	it('an empty list reloads a first page of its usual size, not one row', async () => {
		let rows: string[] = [];
		const { limits, load } = served(() => rows);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		const stop = show(paged);
		limits.length = 0;

		rows = ['a', 'b', 'c'];
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(limits).toEqual([undefined]);
		expect(paged.items).toEqual(['a', 'b']);
		expect(paged.hasMore).toBe(true);
		await stop();
	});

	it('keeps loading while the sentinel is in view after a reload pushed the last row out', async () => {
		vi.useRealTimers();
		let rows = ['a', 'b'];
		const { load } = served(() => rows);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		const el = document.body.appendChild(document.createElement('div'));
		const stop = show(paged);
		const destroy = $effect.root(() => {
			$effect(() => paged.sentinel(el));
		});
		flushSync();
		await new Promise((resolve) => setTimeout(resolve, 100));

		try {
			rows = ['0', 'a', 'b'];
			announce();
			await vi.waitFor(() => expect(paged.items).toEqual(['0', 'a', 'b']), { timeout: 4000 });
		} finally {
			destroy();
			el.remove();
			await stop();
		}
	});

	it('stops loading once the sentinel leaves the view', async () => {
		vi.useRealTimers();
		const rows = Array.from({ length: 400 }, (_, i) => `${1000 + i}`);
		const { calls, delay, load } = served(() => rows);
		delay.ms = 20;
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		const el = document.body.appendChild(document.createElement('div'));
		const destroy = $effect.root(() => {
			$effect(() => paged.sentinel(el));
		});
		flushSync();

		try {
			await vi.waitFor(() => expect(calls.length).toBeGreaterThan(3));
			el.style.cssText = 'position: absolute; top: 100000px';
			await new Promise((resolve) => setTimeout(resolve, 200));
			const read = calls.length;
			await new Promise((resolve) => setTimeout(resolve, 300));
			expect(calls.length - read).toBeLessThanOrEqual(1);
		} finally {
			destroy();
			el.remove();
		}
	});

	it('reloads once for a burst', async () => {
		const { calls, load } = served(() => ['a', 'b']);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
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
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
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
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
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
		const paged = new Paged(await load(), flaky, '1', DEPENDS.tracks, catalog.changes);
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
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		expect(listeners.size).toBe(0);
		expect(paged.items).toEqual(['a']);
		expect(listeners.size).toBe(0);
	});
});

describe('loading more', () => {
	it('is one read at a time, so two asks do not repeat a page', async () => {
		const { delay, load } = served(() => ['a', 'b', 'c', 'd', 'e', 'f']);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		delay.ms = 100;
		const asks = Promise.all([paged.more(), paged.more()]);
		await vi.advanceTimersByTimeAsync(500);
		await asks;
		expect(paged.items).toEqual(['a', 'b', 'c', 'd']);
	});

	it('waits for a reload under way rather than running beside it', async () => {
		const { delay, load } = served(() => ['a', 'b', 'c', 'd', 'e', 'f']);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
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
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
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

describe('loading more', () => {
	it('makes a list that is displayed grow', async () => {
		const { load } = served(() => ['a', 'b', 'c', 'd']);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		const seen: number[] = [];
		const destroy = $effect.root(() => {
			$effect(() => void seen.push(paged.items.length));
		});
		flushSync();
		await paged.more();
		flushSync();
		expect(seen).toEqual([2, 4]);
		destroy();
		await Promise.resolve();
	});
});

describe('the kinds of change a list depends on', () => {
	it('ignores a change to a playlist that is not Liked Songs, and reloads for a like', async () => {
		const { calls, load } = served(() => ['a', 'b', 'c']);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		const stop = show(paged);
		calls.length = 0;

		announce('1', ['playlists']);
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 2);
		expect(calls).toEqual([]);

		// A like is a change to Liked Songs, which is a playlist too.
		announce('1', ['liked', 'playlists']);
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(calls).toEqual([undefined]);
		await stop();
	});

	it("reloads a playlists list for a playlist's change and not for another list's kinds", async () => {
		const { calls, load } = served(() => ['a', 'b']);
		const paged = new Paged(await load(), load, '1', DEPENDS.playlists, catalog.changes);
		const stop = show(paged);
		calls.length = 0;

		announce('1', ['liked']);
		announce('1', ['albums', 'artists', 'tags']);
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 2);
		expect(calls).toEqual([]);

		announce('1', ['playlists']);
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(calls).toEqual([undefined]);
		await stop();
	});

	it('does the same for a single read', async () => {
		let reads = 0;
		const live = new Live(
			{ name: 'a' },
			async () => ({ name: String(++reads) }),
			'1',
			DEPENDS.tags,
			catalog.changes
		);
		const stop = show(live);

		announce('1', ['playlists', 'liked', 'albums']);
		await vi.advanceTimersByTimeAsync(RELOAD_MS * 2);
		expect(reads).toBe(0);

		announce('1', ['tags']);
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(reads).toBe(1);
		await stop();
	});
});

describe('a change announced between a read and the page showing it', () => {
	it('reloads a list once for a relevant change, and a single read too', async () => {
		let rows = ['a', 'b'];
		const { calls, load } = served(() => rows);
		const since = catalog.changes;
		const first = await load();
		let reads = 0;
		const live = new Live(
			{ name: 'a' },
			async () => ({ name: String(++reads) }),
			'1',
			DEPENDS.tags,
			since
		);
		rows = ['0', 'a', 'b'];
		announce('1', ['tracks']);
		const paged = new Paged(first, load, '1', DEPENDS.tracks, since);
		calls.length = 0;
		const stop = show(paged);
		const stopLive = show(live);

		await vi.advanceTimersByTimeAsync(RELOAD_MS * 3);
		expect(calls).toEqual([undefined]);
		expect(paged.items).toEqual(['0', 'a']);
		expect(reads).toBe(1);
		await stop();
		await stopLive();
	});

	it('does not reload when nothing, another kind or another library was announced', async () => {
		const { calls, load } = served(() => ['a', 'b']);
		const since = catalog.changes;
		const first = await load();
		let reads = 0;
		const live = new Live(
			{ name: 'a' },
			async () => ({ name: String(++reads) }),
			'1',
			DEPENDS.tags,
			since
		);
		announce('1', ['playlists', 'liked']);
		announce('2', ['tracks', 'tags']);
		const paged = new Paged(first, load, '1', DEPENDS.albums, since);
		calls.length = 0;
		const stop = show(paged);
		const stopLive = show(live);

		await vi.advanceTimersByTimeAsync(RELOAD_MS * 3);
		expect(calls).toEqual([]);
		expect(reads).toBe(0);
		await stop();
		await stopLive();
	});

	it('does not reload for a change announced before the read', async () => {
		const { calls, load } = served(() => ['a', 'b']);
		announce('1', ['tracks']);
		const paged = new Paged(await load(), load, '1', DEPENDS.tracks, catalog.changes);
		calls.length = 0;
		const stop = show(paged);

		await vi.advanceTimersByTimeAsync(RELOAD_MS * 3);
		expect(calls).toEqual([]);
		await stop();
	});
});

describe('a reload comparing what it reads', () => {
	/** `count` rows that are objects, each read made of new ones, served as the API does. */
	function objects(count: () => number, edited: () => number[]) {
		const load = async (cursor?: string, limit = 1200): Promise<Page<{ n: number; v: number }>> => {
			const from = cursor === undefined ? 0 : Number(cursor) + 1;
			const items = Array.from(
				{ length: Math.max(0, Math.min(limit, count() - from)) },
				(_, i) => ({
					n: from + i,
					v: edited().includes(from + i) ? 1 : 0
				})
			);
			const more = from + items.length < count();
			return { items, nextCursor: more ? String(from + items.length - 1) : null, total: count() };
		};
		return load;
	}

	it('keeps the array it has when no page differs', async () => {
		const load = objects(
			() => 2500,
			() => []
		);
		const paged = new Paged(
			await load(undefined, 2500),
			load,
			'1',
			DEPENDS.tracks,
			catalog.changes
		);
		const stop = show(paged);
		const shown = paged.items;

		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(paged.items).toBe(shown);
		await stop();
	});

	it('stops comparing at the first differing page: the rows before it stay, those after are as read', async () => {
		let edited: number[] = [];
		const load = objects(
			() => 2500,
			() => edited
		);
		const paged = new Paged(
			await load(undefined, 2500),
			load,
			'1',
			DEPENDS.tracks,
			catalog.changes
		);
		const stop = show(paged);
		const shown = paged.items;

		// A row in the second page of 1000 changes, and another in the third: the first page is kept
		// as shown, and from the second on the rows are new, those equal by value included.
		edited = [1500, 2200];
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		const now = paged.items;
		expect(now).not.toBe(shown);
		expect(now).toHaveLength(2500);
		expect(now[1500]).toEqual({ n: 1500, v: 1 });
		expect(now[2200]).toEqual({ n: 2200, v: 1 });
		expect(now[999]).toBe(shown[999]);
		expect(now[1000]).not.toBe(shown[1000]);
		expect(now[2400]).not.toBe(shown[2400]);
		expect(now[2400]).toEqual(shown[2400]);
		await stop();
	});

	it('keeps the rows of a list that only lost its end, in a new array', async () => {
		let count = 2500;
		const load = objects(
			() => count,
			() => []
		);
		const paged = new Paged(
			await load(undefined, 2500),
			load,
			'1',
			DEPENDS.tracks,
			catalog.changes
		);
		const stop = show(paged);
		const shown = paged.items;

		count = 1500;
		announce();
		await vi.advanceTimersByTimeAsync(RELOAD_MS);
		expect(paged.items).toHaveLength(1500);
		expect(paged.items[1499]).toBe(shown[1499]);
		await stop();
	});
});

describe('a single read that reads again', () => {
	it('shows the new read, and keeps the old one by identity when it is the same', async () => {
		const first = { name: 'a' };
		let now = { name: 'a' };
		const live = new Live(first, async () => now, '1', DEPENDS.tracks, catalog.changes);
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
