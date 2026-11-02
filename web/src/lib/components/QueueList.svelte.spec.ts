import { page, userEvent } from 'vitest/browser';
import { beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import type { SentEdit, TrackSummary } from '$lib/api/types';
import type { QueueRow } from '$lib/queue/protocol';

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
	player: { jump: (() => {}) as (item: string) => void },
	/** The rows of Up Next, in order. */
	upNext: [] as QueueRow[],
	/** The tracks the lookup gives, by ID. */
	tracks: new Map<string, unknown>(),
	/** While set, a lookup that starts waits for it. */
	gate: null as Promise<void> | null
}));

vi.mock('$lib/queue/client.svelte', () => ({ queue: fake.queue }));
vi.mock('$lib/player.svelte', () => ({ player: fake.player }));
vi.mock('$lib/queue/summaries', async (original) => ({
	...(await original<typeof import('$lib/queue/summaries')>()),
	lookup: async (refs: { libraryId: string; trackId: string }[]) => {
		await fake.gate;
		return new Map(
			refs.map((r) => [`${r.libraryId}/${r.trackId}`, fake.tracks.get(r.trackId) ?? null])
		);
	}
}));

const queue = $state(fake.queue);
Object.assign(fake, { queue });
queue.rows = async (_section, offset, limit) => ({
	rows: fake.upNext.slice(offset, offset + limit)
});

const { default: QueueList } = await import('./QueueList.svelte');

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

/** The Up Next list. */
const list = () => page.getByRole('list', { name: 'Up Next' });
/** The row of `item`, once it is drawn. */
const row = (item: string) =>
	page.elementLocator(document.querySelector(`[data-item="${item}"]`) as Element);

beforeEach(() => {
	vi.restoreAllMocks();
	fake.tracks = new Map([0, 1, 2, 3].map((n) => [`t${n}`, summary(`t${n}`, `Song ${n}`)]));
	fake.queue.queueId = 'q1';
	fake.queue.refusal = null;
});

/** Renders the list in a box with a height, as the panel gives it. */
function mount() {
	return render(QueueList, {}, { baseElement: document.body });
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
