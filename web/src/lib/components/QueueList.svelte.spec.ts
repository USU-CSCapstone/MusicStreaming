import { page, userEvent } from 'vitest/browser';
import { beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { SentEdit, TrackSummary } from '$lib/api/types';
import type { QueueRow } from '$lib/queue/protocol';
import { realtime } from '$lib/realtime.svelte';

const fake = vi.hoisted(() => ({
	queue: {
		queueId: 'q1' as string | null,
		revision: 1,
		current: 'k.0' as string | null,
		counts: { manual: 0, upNext: 0, history: 0 },
		refusal: null as unknown,
		edit: (() => {}) as (edit: SentEdit) => void,
		rows: (async () => ({ rows: [] })) as (
			section: string,
			offset: number,
			limit: number
		) => Promise<{ rows: QueueRow[] }>
	},
	player: {
		jump: (() => {}) as (item: string) => void,
		rewind: (() => {}) as (play: { play: number; item: string }) => void
	},
	/** The rows of Up Next, in order. */
	upNext: [] as QueueRow[],
	/** The rows of History, newest first. */
	history: [] as QueueRow[],
	/** The tracks the lookup gives, by ID. */
	tracks: new Map<string, unknown>(),
	/** While set, a lookup that starts waits for it. */
	gate: null as Promise<void> | null,
	/** While set, a lookup fails: it gives nothing. */
	failing: false
}));

vi.mock('$lib/queue/client.svelte', () => ({ queue: fake.queue }));
vi.mock('$lib/player.svelte', () => ({ player: fake.player }));
vi.mock('$lib/queue/summaries', async (original) => ({
	...(await original<typeof import('$lib/queue/summaries')>()),
	lookup: async (refs: { libraryId: string; trackId: string }[]) => {
		await fake.gate;
		if (fake.failing) return new Map();
		return new Map(
			refs.map((r) => [`${r.libraryId}/${r.trackId}`, fake.tracks.get(r.trackId) ?? null])
		);
	}
}));

const queue = $state(fake.queue);
Object.assign(fake, { queue });
queue.rows = async (section, offset, limit) => ({
	rows: (section === 'history' ? fake.history : fake.upNext).slice(offset, offset + limit)
});

const { default: QueueList, forgetShown } = await import('./QueueList.svelte');
const { carried } = await import('$lib/dragging');
const { placing } = await import('$lib/queueing.svelte');

/** A track summary with the given ID and title. */
const summary = (id: string, title: string) =>
	({
		id,
		title,
		durationUs: 180_000_000,
		artists: [{ id: '1', name: 'Aurora Lane' }],
		album: { id: '9', title: 'Signal', image: null },
		availability: 'available'
	}) as unknown as TrackSummary;

/** A known row of item `k.<n>` whose track is `t<n>`. */
const known = (n: number): QueueRow => ({
	item: `k.${n}`,
	libraryId: '11',
	status: 'known',
	track: `t${n}`
});

/** Sets Up Next to `rows`, the first `manual` of them in the Manual Queue. */
function setUpNext(rows: QueueRow[], manual = 0) {
	fake.upNext = rows;
	fake.queue.counts = { manual, upNext: rows.length, history: 0 };
	fake.queue.revision++;
}

/** A known History row for play `play` of item `k.<n>`. */
const played = (play: number, n: number): QueueRow => ({ ...known(n), play });

/** Sets History to `rows`, newest first. */
function setHistory(rows: QueueRow[]) {
	fake.history = rows;
	fake.queue.counts = { manual: 0, upNext: 0, history: rows.length };
	fake.queue.revision++;
}

/** The History list. */
const historyList = () => page.getByRole('list', { name: 'History' });

/** The Up Next list. */
const list = () => page.getByRole('list', { name: 'Up Next' });
/** The row of `item`, once it is drawn. */
const row = (item: string) =>
	page.elementLocator(document.querySelector(`[data-item="${item}"]`) as Element);

beforeEach(() => {
	vi.restoreAllMocks();
	forgetShown();
	fake.tracks = new Map([0, 1, 2, 3].map((n) => [`t${n}`, summary(`t${n}`, `Song ${n}`)]));
	fake.queue.queueId = 'q1';
	fake.queue.refusal = null;
	fake.failing = false;
	realtime.connected = false;
});

/** Renders the list of `tab`. */
function mount(tab: 'upNext' | 'history' = 'upNext') {
	return render(QueueList, { tab }, { baseElement: document.body });
}

describe('QueueList', () => {
	it('moves a row with Alt+Arrow from its title, across the end of the Manual Queue', async () => {
		const edits: SentEdit[] = [];
		fake.queue.edit = (e) => {
			edits.push(e);
			fake.queue.revision++;
		};
		setUpNext([known(0), known(1), known(2)], 1);
		mount();
		/** The title button of `Song n`. */
		const title = (n: number) => list().getByRole('button', { name: `Song ${n}`, exact: true });
		await expect.element(title(2)).toBeVisible();

		// Down from the Manual Queue's last row lands before the context's first.
		title(0).element().focus();
		await userEvent.keyboard('{Alt>}{ArrowDown}{/Alt}');
		await expect
			.poll(() => edits.at(-1))
			.toEqual({
				type: 'move',
				items: ['k.0'],
				before: { type: 'before', item: 'k.1' }
			});

		// Up from the context's first row goes to the Manual Queue's end.
		title(1).element().focus();
		await userEvent.keyboard('{Alt>}{ArrowUp}{/Alt}');
		await expect
			.poll(() => edits.at(-1))
			.toEqual({
				type: 'move',
				items: ['k.1'],
				before: { type: 'manualEnd' }
			});

		// Up from the very first row has nowhere to go.
		title(0).element().focus();
		const before = edits.length;
		await userEvent.keyboard('{Alt>}{ArrowUp}{/Alt}');
		expect(edits).toHaveLength(before);
	});

	it('removes a row with Delete, or with its button', async () => {
		const edits: SentEdit[] = [];
		fake.queue.edit = (e) => {
			edits.push(e);
			fake.queue.revision++;
		};
		setUpNext([known(0), known(1)]);
		mount();
		await expect.element(list().getByRole('button', { name: 'Song 1', exact: true })).toBeVisible();
		list().getByRole('button', { name: 'Song 0', exact: true }).element().focus();
		await userEvent.keyboard('{Delete}');
		await page.getByRole('button', { name: 'Remove Song 1' }).click();
		expect(edits).toEqual([
			{ type: 'remove', items: ['k.0'] },
			{ type: 'remove', items: ['k.1'] }
		]);
	});

	it('plays a row when it is clicked', async () => {
		const jump = vi.spyOn(fake.player, 'jump');
		setUpNext([known(0), known(1)]);
		mount();
		await list().getByRole('button', { name: 'Song 1', exact: true }).click();
		expect(jump).toHaveBeenCalledWith('k.1');
	});

	it('draws rows the queue is still learning, or cannot learn, without a track', async () => {
		fake.tracks.delete('t2');
		setUpNext([
			known(0),
			{ item: 'k.1', libraryId: '11', status: 'unknown', track: null },
			{ item: 'k.2', libraryId: '11', status: 'unavailable', track: null },
			known(3)
		]);
		mount();
		await expect.element(list().getByRole('button', { name: 'Song 0', exact: true })).toBeVisible();
		const rows = list().getByRole('listitem');
		await expect.element(rows.nth(0)).toHaveTextContent('Queued by you');
		await expect.element(list().getByRole('button', { name: 'Song 0', exact: true })).toBeVisible();
		const loading = row('k.1');
		await expect.element(loading).toHaveAttribute('data-state', 'loading');
		await expect.element(loading).toHaveAttribute('aria-busy', 'true');
		await expect
			.element(loading.getByRole('button', { name: 'Loading' }))
			.toHaveAttribute('aria-disabled', 'true');
		const gone = row('k.2');
		await expect.element(gone).toHaveAttribute('data-state', 'unavailable');
		await expect.element(gone).toHaveTextContent('Unavailable');
		await expect
			.element(gone.getByRole('button', { name: 'Unavailable track', exact: true }))
			.toHaveAttribute('aria-disabled', 'true');
		// Either can still be removed.
		await expect
			.element(gone.getByRole('button', { name: 'Remove unavailable track' }))
			.toBeEnabled();
		await expect.element(loading.getByRole('button', { name: 'Remove song' })).toBeEnabled();
	});

	it('keeps the arrows, Space and Alt moves working through rows that cannot play', async () => {
		fake.tracks.delete('t1');
		setUpNext([known(0), known(1), known(2)]);
		mount();
		/** The title button of `Song n`. */
		const title = (n: number) => list().getByRole('button', { name: `Song ${n}`, exact: true });
		await expect.element(title(2)).toBeVisible();
		title(0).element().focus();
		await userEvent.keyboard('{ArrowDown}{ArrowDown}');
		expect(document.activeElement).toBe(title(2).element());
		await userEvent.keyboard('{ArrowUp}');
		expect(document.activeElement?.textContent?.trim()).toBe('Unavailable track');
		// Space selects the row that cannot play, and a click on it does not play anything.
		await userEvent.keyboard(' ');
		await expect.element(row('k.1')).toHaveClass('selected');
		const jump = vi.spyOn(fake.player, 'jump');
		(document.activeElement as HTMLElement).click();
		expect(jump).not.toHaveBeenCalled();
	});

	it('moves and removes every selected row together', async () => {
		const edits: SentEdit[] = [];
		fake.queue.edit = (e) => {
			edits.push(e);
			fake.queue.revision++;
		};
		setUpNext([known(0), known(1), known(2)]);
		mount();
		/** The title button of `Song n`. */
		const title = (n: number) => list().getByRole('button', { name: `Song ${n}`, exact: true });
		await expect.element(title(2)).toBeVisible();
		title(2).element().focus();
		await userEvent.keyboard(' ');
		title(1).element().focus();
		await userEvent.keyboard(' ');
		await expect.element(row('k.1')).toHaveClass('selected');
		await expect.element(row('k.2')).toHaveClass('selected');
		await userEvent.keyboard('{Alt>}{ArrowUp}{/Alt}');
		await expect
			.poll(() => edits.at(-1))
			.toEqual({
				type: 'move',
				items: expect.arrayContaining(['k.1', 'k.2']),
				before: { type: 'before', item: 'k.0' }
			});
		await page.getByRole('button', { name: 'Remove Song 1' }).click();
		expect(edits.at(-1)).toEqual({ type: 'remove', items: expect.arrayContaining(['k.1', 'k.2']) });
		expect((edits.at(-1) as { items: string[] }).items).toHaveLength(2);
		// The selection went with them: a later removal is of its own row only.
		await expect.element(row('k.2')).not.toHaveClass('selected');
		await page.getByRole('button', { name: 'Remove Song 0' }).click();
		expect(edits.at(-1)).toEqual({ type: 'remove', items: ['k.0'] });
	});

	it('selects with Ctrl-click and with a press on the grip that does not move', async () => {
		setUpNext([known(0), known(1)]);
		mount();
		await expect.element(list().getByRole('button', { name: 'Song 1', exact: true })).toBeVisible();
		await list()
			.getByRole('button', { name: 'Song 0', exact: true })
			.click({ modifiers: ['Control'] });
		await expect.element(row('k.0')).toHaveClass('selected');
		const grip = row('k.1').element().querySelector('.grip') as HTMLElement;
		const box = grip.getBoundingClientRect();
		const at = { clientX: box.x + 4, clientY: box.y + 4, pointerId: 1, button: 0, bubbles: true };
		grip.dispatchEvent(new PointerEvent('pointerdown', at));
		grip.dispatchEvent(new PointerEvent('pointerup', at));
		await expect.element(row('k.1')).toHaveClass('selected');
	});

	it('shows a track the lookup does not give, or that is missing, as unavailable', async () => {
		fake.tracks.delete('t1');
		fake.tracks.set('t2', { ...summary('t2', 'Song 2'), availability: 'missing' });
		setUpNext([known(0), known(1), known(2)]);
		mount();
		await expect.poll(() => document.querySelector('[data-item="k.2"]')).not.toBeNull();
		await expect.element(row('k.0')).toHaveAttribute('data-state', 'ready');
		await expect.element(row('k.1')).toHaveAttribute('data-state', 'unavailable');
		await expect.element(row('k.2')).toHaveAttribute('data-state', 'unavailable');
	});

	it('looks again for the tracks a failed lookup left out once the connection returns', async () => {
		fake.failing = true;
		setUpNext([known(0), known(1)]);
		mount();
		await expect.poll(() => document.querySelector('[data-item="k.1"]')).not.toBeNull();
		await expect.element(row('k.0')).toHaveAttribute('data-state', 'loading');

		fake.failing = false;
		realtime.connected = true;
		await expect.element(row('k.0')).toHaveAttribute('data-state', 'ready');
		await expect.element(row('k.1')).toHaveAttribute('data-state', 'ready');
	});

	it('sends nothing for a move past either end', async () => {
		const edits: SentEdit[] = [];
		fake.queue.edit = (e) => {
			edits.push(e);
			fake.queue.revision++;
		};
		setUpNext([known(0), known(1)]);
		mount();
		const last = list().getByRole('button', { name: 'Song 1', exact: true });
		await expect.element(last).toBeVisible();
		last.element().focus();
		await userEvent.keyboard('{Alt>}{ArrowDown}{/Alt}');
		await new Promise((r) => setTimeout(r, 50));
		expect(edits).toEqual([]);
	});

	it('moves one place per press when pressed faster than the list updates', async () => {
		const edits: SentEdit[] = [];
		fake.queue.edit = (e) => {
			edits.push(e);
			if (e.type === 'remove') {
				fake.upNext = fake.upNext.filter((r) => !e.items.includes(r.item));
				fake.queue.counts = { ...fake.queue.counts, upNext: fake.upNext.length };
			}
			if (e.type === 'move' && e.before.type === 'before') {
				const moved = fake.upNext.filter((r) => e.items.includes(r.item));
				const rest = fake.upNext.filter((r) => !e.items.includes(r.item));
				const at = rest.findIndex((r) => r.item === (e.before as { item: string }).item);
				fake.upNext = [...rest.slice(0, at), ...moved, ...rest.slice(at)];
			}
			fake.queue.revision++;
		};
		setUpNext([known(0), known(1), known(2), known(3)]);
		mount();
		const first = list().getByRole('button', { name: 'Song 0', exact: true });
		await expect.element(first).toBeVisible();
		first.element().focus();
		/** Presses Alt+ArrowDown on Song 0's title. */
		const press = () =>
			first
				.element()
				.dispatchEvent(
					new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true })
				);
		// Both presses in one tick, before the list can have caught up with the first.
		press();
		press();
		await expect.poll(() => edits.length).toBe(2);
		// Song 0 went before Song 2, then, with the list caught up, before Song 3.
		expect(edits[1]).toMatchObject({ items: ['k.0'], before: { type: 'before', item: 'k.3' } });
	});

	it('puts focus on the next row after Delete removes the focused one', async () => {
		const edits: SentEdit[] = [];
		fake.queue.edit = (e) => {
			edits.push(e);
			if (e.type === 'remove') {
				fake.upNext = fake.upNext.filter((r) => !e.items.includes(r.item));
				fake.queue.counts = { ...fake.queue.counts, upNext: fake.upNext.length };
			}
			fake.queue.revision++;
		};
		setUpNext([known(0), known(1), known(2)]);
		mount();
		/** The title button of `Song n`. */
		const title = (n: number) => list().getByRole('button', { name: `Song ${n}`, exact: true });
		await expect.element(title(2)).toBeVisible();
		title(1).element().focus();
		await userEvent.keyboard('{Delete}');
		await expect.poll(() => document.activeElement).toBe(title(2).element());
		// With no row after it, the one before.
		await userEvent.keyboard('{Delete}');
		await expect.poll(() => document.activeElement).toBe(title(0).element());
	});

	it('does not let a lookup that lands late drop the rows now in view', async () => {
		fake.tracks = new Map(
			Array.from({ length: 1300 }, (_, n) => [`t${n}`, summary(`t${n}`, `Song ${n}`)])
		);
		setUpNext(Array.from({ length: 1300 }, (_, n) => known(n)));
		const box = document.createElement('style');
		box.textContent = '.scroll { height: 400px; flex: none; }';
		document.head.append(box);
		onTestFinished(() => {
			box.remove();
			fake.gate = null;
		});
		mount();
		const scroller = document.querySelector('.scroll') as HTMLElement;
		await expect.element(list().getByRole('button', { name: 'Song 0', exact: true })).toBeVisible();
		/** Waits for the list to read and draw what it has been given. */
		const settle = () => new Promise((r) => setTimeout(r, 50));
		for (let row = 18; row < 980; row += 18) {
			scroller.scrollTop = row * 56;
			await settle();
		}
		// A lookup for one view is held while the user scrolls on to another, whose lookup lands
		// first and takes the cache past its limit; then the held one lands.
		let release!: () => void;
		fake.gate = new Promise((r) => (release = r));
		scroller.scrollTop = 1010 * 56;
		await settle();
		fake.gate = null;
		scroller.scrollTop = 1150 * 56;
		await expect
			.poll(() => document.querySelectorAll('[data-state="ready"]').length)
			.toBeGreaterThan(5);
		release();
		await settle();
		await settle();
		expect(document.querySelectorAll('[data-state="loading"]')).toHaveLength(0);
		expect(document.querySelectorAll('[data-state="ready"]').length).toBeGreaterThan(5);
	}, 30_000);

	it('keeps the tracks in view when a long scroll has filled the cache', async () => {
		fake.tracks = new Map(
			Array.from({ length: 1200 }, (_, n) => [`t${n}`, summary(`t${n}`, `Song ${n}`)])
		);
		setUpNext(Array.from({ length: 1200 }, (_, n) => known(n)));
		// A box 400px tall, as the panel gives, so only the rows in view are drawn.
		const box = document.createElement('style');
		box.textContent = '.scroll { height: 400px; flex: none; }';
		document.head.append(box);
		onTestFinished(() => box.remove());
		mount();
		const scroller = document.querySelector('.scroll') as HTMLElement;
		await expect.element(list().getByRole('button', { name: 'Song 0', exact: true })).toBeVisible();
		/** Waits for the list to read and draw what it has been given. */
		const settle = () => new Promise((r) => setTimeout(r, 50));
		// Scrolling on in jumps fills the cache with about a thousand tracks.
		for (let row = 18; row < 940; row += 18) {
			scroller.scrollTop = row * 56;
			await settle();
		}
		// Then a row at a time, as the cache passes its limit: the rows in view keep their tracks.
		for (let k = 0; k < 120; k++) {
			scroller.scrollTop += 56;
			await settle();
			expect(document.querySelectorAll('[data-state="loading"]'), `after ${k} rows`).toHaveLength(
				0
			);
		}
		expect(document.querySelectorAll('[data-state="ready"]').length).toBeGreaterThan(5);
	}, 30_000);

	it('says so when there is no queue', async () => {
		fake.queue.queueId = null;
		mount();
		await expect.element(page.getByText('Play something to start a queue.')).toBeVisible();
	});
});

describe('QueueList adding', () => {
	/** What a mouse drags from a view: `tracks` of its own, unless it has a `recipe`. */
	const album = {
		label: '“Signal”',
		count: 2,
		refs: [],
		tracks: async () => [summary('t8', 'Eight'), summary('t9', 'Nine')],
		recipe: { type: 'album', albumId: '31', sort: 'album', order: 'asc' } as const
	};
	const two = { ...album, recipe: undefined };
	let edits: SentEdit[];

	beforeEach(() => {
		edits = [];
		// The worker answers a moment after the edit is sent, with the queue as it then is.
		fake.queue.edit = (e) => {
			edits.push(e);
			setTimeout(() => fake.queue.revision++, 0);
		};
		// Up Next: one Manual row (rows 0 heading, 1 item), then the context's heading (2) and two rows.
		setUpNext([known(0), known(1), known(2)], 1);
		placing.what = null;
		carried.what = null;
	});

	/** Drops what is carried on the line before row `gap` of Up Next. */
	async function dropAt(gap: number, type = 'drop') {
		const scroller = document.querySelector('.scroll')!;
		const top = scroller.getBoundingClientRect().top;
		const event = new DragEvent(type, {
			bubbles: true,
			cancelable: true,
			clientY: top + gap * 56
		});
		scroller.dispatchEvent(event);
		return event;
	}

	it('drop inserts at the anchor', async () => {
		mount();
		await expect.element(list().getByRole('button', { name: 'Song 2', exact: true })).toBeVisible();
		carried.what = two;
		carried.libraryId = '11';

		// Before the context's last row: just ahead of that item.
		await dropAt(4);
		await expect
			.poll(() => edits.at(-1))
			.toMatchObject({
				type: 'add',
				source: { libraryId: '11', tracks: ['t8', 't9'] },
				before: { type: 'before', item: 'k.2' }
			});
		expect(carried.what, 'let go of once dropped').toBeNull();

		// Above the context's heading is the Manual Queue's end, and above all the rows its front.
		carried.what = two;
		await dropAt(2);
		await expect.poll(() => edits.at(-1)).toMatchObject({ before: { type: 'manualEnd' } });
		carried.what = two;
		await dropAt(0);
		await expect.poll(() => edits.at(-1)).toMatchObject({ before: { type: 'manualStart' } });
		// Below the last row is the context's end.
		carried.what = two;
		await dropAt(5);
		await expect.poll(() => edits.at(-1)).toMatchObject({ before: { type: 'contextEnd' } });
	});

	it('drops a collection by its recipe, and draws the line while it is over the list', async () => {
		mount();
		await expect.element(list().getByRole('button', { name: 'Song 2', exact: true })).toBeVisible();
		carried.what = album;
		carried.libraryId = '11';
		const over = await dropAt(3, 'dragover');
		expect(over.defaultPrevented, 'accepts the drag').toBe(true);
		expect(document.querySelector('.drop')).not.toBeNull();
		await dropAt(3);
		await expect
			.poll(() => edits.at(-1))
			.toEqual({
				type: 'add',
				source: {
					key: expect.any(String),
					libraryId: '11',
					recipe: { type: 'album', albumId: '31', sort: 'album', order: 'asc' }
				},
				before: { type: 'before', item: 'k.1' }
			});
		expect(document.querySelector('.drop')).toBeNull();
	});

	it('takes nothing dropped on History, or that is not carried from a view', async () => {
		mount('history');
		const dragover = await dropAt(1, 'dragover');
		carried.what = two;
		await dropAt(1);
		expect(dragover.defaultPrevented).toBe(false);
		expect(edits).toEqual([]);
	});

	it('adds at a point chosen with the keyboard, before a row or at either end', async () => {
		mount();
		await expect.element(list().getByRole('button', { name: 'Song 2', exact: true })).toBeVisible();
		placing.what = two;
		placing.libraryId = '11';
		const group = page.getByRole('group', { name: 'Choose where to add' });
		await expect.element(group).toBeVisible();
		await expect.element(group.getByRole('button', { name: 'Add to the end' })).toHaveFocus();

		// Before the context's first row.
		await list()
			.getByRole('button', { name: /^Add .* before Song 1$/ })
			.click();
		await expect
			.poll(() => edits.at(-1))
			.toMatchObject({
				type: 'add',
				source: { tracks: ['t8', 't9'] },
				before: { type: 'before', item: 'k.1' }
			});
		await expect.element(group).not.toBeInTheDocument();
		expect(placing.what).toBeNull();

		// At the end of the Manual Queue, by its heading.
		placing.what = two;
		await list()
			.getByRole('button', { name: /last in Queued by you$/ })
			.click();
		await expect.poll(() => edits.at(-1)).toMatchObject({ before: { type: 'manualEnd' } });
		// At the front of it.
		placing.what = two;
		await list()
			.getByRole('button', { name: /first in Queued by you$/ })
			.click();
		await expect.poll(() => edits.at(-1)).toMatchObject({ before: { type: 'manualStart' } });
		// And at the end of Up Next.
		placing.what = two;
		await page.getByRole('button', { name: 'Add to the end' }).click();
		await expect.poll(() => edits.at(-1)).toMatchObject({ before: { type: 'contextEnd' } });
	});

	it('adds before a row when its title is pressed while choosing, and never jumps', async () => {
		const jumps: string[] = [];
		fake.player.jump = (item) => void jumps.push(item);
		mount();
		await expect.element(list().getByRole('button', { name: 'Song 2', exact: true })).toBeVisible();
		placing.what = two;
		await list().getByRole('button', { name: 'Song 2', exact: true }).click();
		await expect
			.poll(() => edits.at(-1))
			.toMatchObject({ type: 'add', before: { type: 'before', item: 'k.2' } });
		expect(jumps).toEqual([]);
	});

	it('gives up choosing a place with Escape or Cancel, adding nothing', async () => {
		mount();
		placing.what = two;
		await expect.element(page.getByRole('group', { name: 'Choose where to add' })).toBeVisible();
		await userEvent.keyboard('{Escape}');
		expect(placing.what).toBeNull();
		placing.what = two;
		await page.getByRole('button', { name: 'Cancel' }).click();
		expect(placing.what).toBeNull();
		expect(edits).toEqual([]);
	});
});

describe('QueueList History', () => {
	/** Records the edits sent, as the queue applies them: the rows are read again after each. */
	function record() {
		const edits: SentEdit[] = [];
		fake.queue.edit = (e) => {
			edits.push(e);
			fake.queue.revision++;
		};
		return edits;
	}

	it('lists plays newest first, and rewinds to the one clicked', async () => {
		const rewind = vi.spyOn(fake.player, 'rewind');
		setHistory([played(7, 2), played(5, 1), played(4, 0)]);
		mount('history');
		const titles = () =>
			[...document.querySelectorAll('[data-item] .title')].map((e) => e.textContent?.trim());
		await expect.poll(titles).toEqual(['Song 2', 'Song 1', 'Song 0']);
		await historyList().getByRole('button', { name: 'Song 1', exact: true }).click();
		expect(rewind).toHaveBeenCalledWith({ play: 5, item: 'k.1' });
		// Enter on a title does the same, from the keyboard.
		historyList().getByRole('button', { name: 'Song 0', exact: true }).element().focus();
		await userEvent.keyboard('{Enter}');
		expect(rewind).toHaveBeenLastCalledWith({ play: 4, item: 'k.0' });
	});

	it('keeps two plays of one item apart', async () => {
		const edits = record();
		setHistory([played(9, 0), played(3, 0)]);
		mount('history');
		await expect.element(historyList().getByRole('listitem').nth(1)).toBeVisible();
		await page.getByRole('button', { name: 'Remove Song 0' }).nth(1).click();
		expect(edits).toEqual([{ type: 'removePlays', plays: [{ play: 3, item: 'k.0' }] }]);
	});

	it('removes a play with Delete, or with its button, and the selected plays together', async () => {
		const edits = record();
		setHistory([played(7, 3), played(6, 2), played(5, 1), played(4, 0)]);
		mount('history');
		const title = (n: number) =>
			historyList().getByRole('button', { name: `Song ${n}`, exact: true });
		await expect.element(title(0)).toBeVisible();
		title(3).element().focus();
		await userEvent.keyboard('{Delete}');
		// Re-read after the edit: Song 3 is gone from the drawn rows.
		setHistory([played(6, 2), played(5, 1), played(4, 0)]);
		await expect.poll(() => document.activeElement).toBe(title(2).element());
		await page.getByRole('button', { name: 'Remove Song 2' }).click();
		expect(edits).toEqual([
			{ type: 'removePlays', plays: [{ play: 7, item: 'k.3' }] },
			{ type: 'removePlays', plays: [{ play: 6, item: 'k.2' }] }
		]);

		// Select two with the check handle and with Space, then one press removes both.
		setHistory([played(5, 1), played(4, 0)]);
		await expect.element(title(1)).toBeVisible();
		await page.getByRole('button', { name: 'Select Song 1' }).click();
		title(0).element().focus();
		await userEvent.keyboard(' ');
		await expect.element(row('5:k.1')).toHaveClass('selected');
		await expect.element(row('4:k.0')).toHaveClass('selected');
		await page.getByRole('button', { name: 'Remove Song 0' }).click();
		expect(edits.at(-1)).toEqual({
			type: 'removePlays',
			plays: [
				{ play: 5, item: 'k.1' },
				{ play: 4, item: 'k.0' }
			]
		});
	});

	it('queues a played track next, under a key of its own', async () => {
		const edits = record();
		setHistory([played(7, 2), played(5, 1)]);
		mount('history');
		const title = (n: number) =>
			historyList().getByRole('button', { name: `Song ${n}`, exact: true });
		await expect.element(title(2)).toBeVisible();
		await expect.element(title(1)).toBeVisible();
		title(1).element().focus();
		await page.getByRole('button', { name: 'Queue Song 1 next' }).click();
		// The edit is applied: the played song leaves History, and focus stays in the list.
		setHistory([played(7, 2)]);
		await expect.poll(() => document.activeElement).toBe(title(2).element());
		expect(edits).toEqual([
			{
				type: 'requeue',
				play: { play: 5, item: 'k.1' },
				key: expect.stringMatching(/^[0-9a-f-]{36}$/)
			}
		]);
	});

	it('draws plays that are unknown or unavailable without a track, and does not rewind to them', async () => {
		const rewind = vi.spyOn(fake.player, 'rewind');
		const edits = record();
		fake.tracks.delete('t2');
		setHistory([
			played(9, 0),
			{ item: 'k.1', libraryId: '11', status: 'unknown', track: null, play: 8 },
			{ item: 'k.2', libraryId: '11', status: 'unavailable', track: null, play: 7 }
		]);
		mount('history');
		await expect.poll(() => document.querySelector('[data-item="7:k.2"]')).not.toBeNull();
		await expect.element(row('9:k.0')).toHaveAttribute('data-state', 'ready');
		await expect.element(row('8:k.1')).toHaveAttribute('data-state', 'loading');
		const gone = row('7:k.2');
		await expect.element(gone).toHaveAttribute('data-state', 'unavailable');
		await expect.element(gone).toHaveTextContent('Unavailable');
		const title = gone.getByRole('button', { name: 'Unavailable track', exact: true });
		await expect.element(title).toHaveAttribute('aria-disabled', 'true');
		(title.element() as HTMLElement).click();
		expect(rewind).not.toHaveBeenCalled();
		// It cannot be put back in Up Next, but can be removed.
		await expect.element(gone.getByRole('button', { name: /Queue .* next/ })).toBeDisabled();
		await gone.getByRole('button', { name: 'Remove unavailable track' }).click();
		expect(edits).toEqual([{ type: 'removePlays', plays: [{ play: 7, item: 'k.2' }] }]);
	});

	it('says so when nothing has played', async () => {
		setHistory([]);
		mount('history');
		await expect.element(page.getByText('Nothing has played yet.')).toBeVisible();
	});

	it('puts focus on the History tab when removing the last play leaves no row', async () => {
		fake.queue.edit = (e) => {
			if (e.type === 'removePlays') setHistory([]);
		};
		// The panel's tab, which the list does not draw.
		const tab = document.createElement('button');
		tab.id = 'queue-tab-history';
		document.body.append(tab);
		onTestFinished(() => tab.remove());
		setHistory([played(3, 0)]);
		mount('history');
		const remove = page.getByRole('button', { name: 'Remove Song 0' });
		await expect.element(remove).toBeVisible();
		(remove.element() as HTMLElement).focus();
		await userEvent.keyboard('{Delete}');

		await expect.poll(() => document.activeElement).toBe(tab);
		await expect.element(page.getByText('Nothing has played yet.')).toBeVisible();
	});
});
