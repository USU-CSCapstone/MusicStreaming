<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import type { QueueAnchor, TrackSummary } from '$lib/api/types';
	import { carried, drop, edgeStep } from '$lib/dragging';
	import { artistName, formatDuration, plural } from '$lib/format';
	import { queue } from '$lib/queue/client.svelte';
	import { anchorAt, MAX_SPACE, ROW, upNextPieces, upNextRow, upNextRows } from '$lib/queue/layout';
	import { lookup } from '$lib/queue/summaries';
	import { addToQueue } from '$lib/queueing';
	import Artwork from './Artwork.svelte';
	import Icon from './Icon.svelte';

	// One tab of the queue panel as a list of rows that are all `ROW` tall, so only the rows in
	// view, and a few beyond, are read from the queue and drawn however long it is
	// (`requirements/performance.md` §6). Up Next shows the Manual Queue apart from the context
	// and can be edited (`requirements/queue.md` §2, §3); History shows what played, most
	// recent first. Every change is an edit the queue client applies at once and sends on.

	let { tab }: { tab: 'up' | 'history' } = $props();

	/** Rows drawn beyond the ones in view, so a scroll shows rows at once. */
	const OVERSCAN = 6;

	const up = $derived(tab === 'up');
	const manual = $derived(queue.counts.manual);
	const upcoming = $derived(queue.counts.upcoming);
	const history = $derived(queue.counts.history);
	const total = $derived(up ? upNextRows(manual, upcoming) : history);

	let scroller = $state<HTMLDivElement>();
	let scrollTop = $state(0);
	let height = $state(0);
	/** The height drawn: a list taller than `MAX_SPACE` scrolls through a scaled range. */
	const space = $derived(Math.min(total * ROW, MAX_SPACE));
	/** Where in the whole list a scroll position is: past `MAX_SPACE`, a pixel scrolls more than one. */
	const at = (top: number) =>
		space > height ? (top * (total * ROW - height)) / (space - height) : top;
	const offset = $derived(at(scrollTop));
	const first = $derived(Math.max(0, Math.floor(offset / ROW) - OVERSCAN));
	const last = $derived(Math.min(total, Math.ceil((offset + height) / ROW) + OVERSCAN));
	/** Where row `v` is drawn, in the scrolled content. */
	const topOf = (v: number) => v * ROW - offset + scrollTop;

	/** The item of each row that has one, and what each item shows: read for the rows in view. */
	let ids = $state.raw(new Map<number, string>());
	let shown = $state.raw(new Map<string, TrackSummary | null>());
	/** Where each item is, the inverse of `ids`. */
	const rowOf = $derived(new Map([...ids].map(([v, item]) => [item, v])));

	/** The rows to draw: a heading or hint at once, an item once it is read. */
	const rows = $derived.by(() => {
		const out: {
			key: string;
			v: number;
			kind: 'heading' | 'hint' | 'item';
			text?: string;
			item?: string;
			/** An item's place among all of the tab's items, from 1. */
			place?: number;
		}[] = [];
		for (let v = first; v < last; v++) {
			const row = up ? upNextRow(v, manual) : ({ kind: 'item' } as const);
			if (row.kind === 'heading') {
				const text = row.section === 'manual' ? 'Queued by you' : 'Up next';
				out.push({ key: `heading-${row.section}`, v, kind: 'heading', text });
			} else if (row.kind === 'hint') {
				out.push({ key: 'hint', v, kind: 'hint' });
			} else if (ids.has(v) && shown.has(ids.get(v)!)) {
				const place = 'list' in row ? (row.list === 'manual' ? 0 : manual) + row.at + 1 : v + 1;
				out.push({ key: ids.get(v)!, v, kind: 'item', item: ids.get(v), place });
			}
		}
		return out;
	});

	/** The queue revision `ids` was read at, so an action knows whether it sees the queue as it is. */
	let idsRevision = 0;
	/** The revision when the last edit was sent: `ids` is stale until one after it is read. */
	let sentAt = -1;
	const fresh = () => idsRevision > sentAt;
	/** Actions waiting for `ids` to catch up with an edit, run in order. */
	const held: (() => Promise<void> | void)[] = [];
	let running = false;

	/** Runs held actions one at a time, each once `ids` shows the queue as the one before left it. */
	async function flush() {
		if (running) return;
		running = true;
		try {
			while (held.length > 0 && fresh()) await held.shift()!();
		} finally {
			running = false;
		}
	}

	/** Runs `action` when the rows drawn are the queue as it is, after the actions before it. */
	function act(action: () => Promise<void> | void) {
		held.push(action);
		void flush();
	}

	/** Sends an edit; rows drawn are stale until the queue's answer to it is read. */
	function send(edit: Parameters<typeof queue.edit>[0]) {
		sentAt = queue.revision;
		queue.edit(edit);
	}

	// An edit the queue refused changes nothing, so there is no change to wait for.
	$effect(() => {
		void queue.refusal;
		sentAt = -1;
		untrack(() => void flush());
	});

	// Reads what is in view. A change to the queue, or a scroll, asks again. The items are known
	// as soon as the queue answers, so a drop can be aimed at them; a row is drawn once its track
	// is known too, so rows do not appear piece by piece. A lookup that is overtaken by a scroll
	// still lands, since the next view may want the same rows.
	$effect(() => {
		const revision = queue.revision;
		const [from, to, isUp] = [first, last, up];
		const [m, u, h] = [manual, upcoming, history];
		let current = true;
		(async () => {
			// eslint-disable-next-line svelte/prefer-svelte-reactivity -- made whole, then replaces `ids`
			const found = new Map<number, string>();
			if (isUp) {
				for (const piece of upNextPieces(from, to, m, u)) {
					const items = await queue.window(piece.list, piece.offset, piece.limit);
					items.forEach((item, k) => found.set(piece.row + k, item));
				}
			} else if (to > from) {
				const items = (await queue.window('history', h - to, to - from)).toReversed();
				items.forEach((item, k) => found.set(from + k, item));
			}
			if (!current) return;
			ids = found;
			idsRevision = revision;
			const summaries = await lookup([...found.values()]);
			const wanted = new Set(ids.values());
			// eslint-disable-next-line svelte/prefer-svelte-reactivity -- replaces `shown`
			const next = new Map<string, TrackSummary | null>();
			for (const [item, track] of shown) if (wanted.has(item)) next.set(item, track);
			for (const [item, one] of summaries) if (wanted.has(item)) next.set(item, one && one.track);
			shown = next;
			await tick();
			void flush();
			refocus();
		})().catch(() => {
			// The queue closed, or the page is going: nothing more to show.
		});
		return () => {
			current = false;
		};
	});

	// A selection belongs to one tab of one queue, and to what has not been played: a song that
	// moved to History is not one to drag or remove with the rest.
	const selected = new SvelteSet<string>();
	$effect(() => {
		void [tab, queue.queueId, queue.current];
		untrack(() => selected.clear());
	});

	/** What the last edit was, for screen readers. */
	let said = $state('');
	/** The item whose title should have focus once its row is drawn again, after a move. */
	let focusItem: string | null = null;
	/** Focus should go to the first title, as after a jump took the focused row away. */
	let focusFirst = false;

	/** The title button of `item`'s row, if the row is drawn. */
	const titleOf = (item: string) =>
		scroller?.querySelector<HTMLElement>(`[data-item="${CSS.escape(item)}"] .title`) ?? null;

	/** Gives focus back to the row an edit moved, which is drawn again in its new place. */
	function refocus() {
		if (!fresh() || scroller?.contains(document.activeElement)) return;
		const item = focusItem;
		focusItem = null;
		if (item) titleOf(item)?.focus();
		else if (focusFirst) scroller?.querySelector<HTMLElement>('.title:not(:disabled)')?.focus();
		focusFirst = false;
	}

	/**
	 * Moves `items` to just before row `gap` of Up Next, unless that leaves them where they are.
	 * The rows the drop looks at are read first if they are not drawn: after a scroll they may
	 * not be, and an unread row is not the end of a section.
	 */
	async function moveTo(items: string[], gap: number) {
		const where = items.map((item) => rowOf.get(item)).filter((v) => v !== undefined);
		const near = Math.min(...where);
		const far = Math.max(...where);
		if (
			where.length === items.length &&
			far - near + 1 === where.length &&
			gap >= near &&
			gap <= far + 1
		) {
			return;
		}
		send({ type: 'move', items, before: await anchorFor(gap, items) });
	}

	/**
	 * What a drop before row `gap` of Up Next anchors to, with the rows it looks at read first if
	 * they are not drawn. `moving` are the items being moved, which it does not anchor to.
	 */
	async function anchorFor(gap: number, moving: string[]): Promise<QueueAnchor> {
		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- local
		const known = new Map(ids);
		for (const piece of upNextPieces(gap, gap + moving.length + 1, manual, upcoming)) {
			if (known.has(piece.row) && known.has(piece.row + piece.limit - 1)) continue;
			const read = await queue.window(piece.list, piece.offset, piece.limit);
			read.forEach((item, k) => known.set(piece.row + k, item));
		}
		return anchorAt(gap, manual, upcoming, (v) => known.get(v), new Set(moving));
	}

	/** The items an action on `item` applies to: the selection if `item` is in it, else `item`. */
	const targets = (item: string) => (selected.has(item) ? [...selected] : [item]);

	/** Alt+↑/↓: the items step one place, across the Manual Queue's end if they reach it. */
	function step(item: string, by: -1 | 1) {
		focusItem = item;
		// Worked out when the rows show the queue as the press before left it.
		act(() => {
			const items = targets(item);
			const where = items.map((i) => rowOf.get(i)).filter((v) => v !== undefined);
			if (where.length === 0) return;
			const gap = by < 0 ? Math.min(...where) - 1 : Math.max(...where) + 2;
			if (gap < 1 || gap > total) return;
			said = `${plural(items.length, 'song')} moved ${by < 0 ? 'up' : 'down'}`;
			return moveTo(items, gap);
		});
	}

	/** Removes the row's item, or the selection it is in, keeping focus in the list. */
	function remove(item: string) {
		const items = targets(item);
		const at = rowOf.get(item) ?? 0;
		const next = ids.get(at + 1) ?? ids.get(at - 1);
		if (next && !items.includes(next)) focusItem = next;
		send({ type: 'remove', items });
		selected.clear();
		said = `${plural(items.length, 'song')} removed`;
	}

	/** Plays `item` now: from Up Next, everything ahead of it goes to History. */
	function jump(item: string) {
		focusFirst = true;
		send({ type: 'jump', item });
	}

	/** Puts a History item at the front of Up Next, where it plays next. */
	function bringBack(item: string) {
		send({ type: 'move', items: [item], before: { type: 'manualStart' } });
		said = 'Song returned to Up Next';
	}

	/** Keys on a row: arrows between rows, Alt+arrows to move, Space to select, Delete to remove. */
	function onrowkey(e: KeyboardEvent, item: string) {
		const key = e.key;
		// Space acts on press and is held back on release; everything else acts once, on press.
		if (e.ctrlKey || e.metaKey || (e.type === 'keyup' && key !== ' ')) return;
		if (key === 'ArrowUp' || key === 'ArrowDown') {
			e.preventDefault();
			const by = key === 'ArrowUp' ? -1 : 1;
			if (e.altKey) {
				if (up) step(item, by);
				return;
			}
			const titles = [...(scroller?.querySelectorAll<HTMLElement>('.title') ?? [])];
			const at = titles.indexOf(e.target as HTMLElement);
			titles[at + by]?.focus();
		} else if (key === 'Delete' || key === 'Backspace') {
			e.preventDefault();
			remove(item);
		} else if (key === ' ' && up && e.target instanceof HTMLElement && e.target.matches('.title')) {
			// The browser would click the button on Space too.
			e.preventDefault();
			if (e.type === 'keydown') toggle(item);
		}
	}

	/** Selects `item`, or lets it go. */
	function toggle(item: string) {
		if (!selected.delete(item)) selected.add(item);
		said = `${plural(selected.size, 'song')} selected`;
	}

	/** A click on a row's title: Ctrl/Cmd selects it; otherwise it plays. */
	function titleClick(e: MouseEvent, item: string) {
		if (up && (e.ctrlKey || e.metaKey)) toggle(item);
		else jump(item);
	}

	// Dragging by the grab handle, with pointer events so a finger drags as a mouse does. The
	// handle takes no scroll from a finger, so touch drags at once. A press on it that does not
	// move selects the row, which is how touch selects. The scroller holds the pointer, since
	// the row it began on may scroll out of existence.
	type Drag = {
		item: string;
		items: string[];
		from: { x: number; y: number };
		/** Where the pointer is. */
		x: number;
		y: number;
		/** Once it has moved far enough to be a drag rather than a press. */
		started: boolean;
		/** The gap the items would drop into: before this row. */
		gap: number;
	};
	let drag = $state<Drag | null>(null);
	let frame = 0;

	/** Begins a press on a row's handle; the pointer moving makes it a drag. */
	function gripDown(e: PointerEvent, item: string) {
		if (e.button !== 0 || !scroller) return;
		e.preventDefault();
		scroller.setPointerCapture(e.pointerId);
		const at = { x: e.clientX, y: e.clientY };
		drag = { item, items: targets(item), from: at, ...at, started: false, gap: 0 };
	}

	/** Follows the pointer once a press has moved. */
	function dragMove(e: PointerEvent) {
		if (!drag) return;
		drag.x = e.clientX;
		drag.y = e.clientY;
		if (!drag.started) {
			if (Math.hypot(e.clientX - drag.from.x, e.clientY - drag.from.y) < 4) return;
			drag.started = true;
			frame = requestAnimationFrame(autoScroll);
		}
		aim();
	}

	/** The gap nearest `clientY`, from the scroll position, as every row is the same height. */
	function gapAt(clientY: number) {
		const y = clientY - scroller!.getBoundingClientRect().top + at(scroller!.scrollTop);
		return Math.max(0, Math.min(total, Math.round(y / ROW)));
	}

	/** The gap nearest the pointer. */
	function aim() {
		if (drag && scroller) drag.gap = gapAt(drag.y);
	}

	/**
	 * Near the top or bottom of the list, scrolls toward it, faster the closer `y`, the pointer's,
	 * gets. Whether it did.
	 */
	function nudge(y: number): boolean {
		const step = edgeStep(y, scroller!.getBoundingClientRect());
		if (step) scroller!.scrollTop += step;
		return step !== 0;
	}

	/** Keeps scrolling while a drag is held near an edge. */
	function autoScroll() {
		if (!drag?.started || !scroller) return;
		if (nudge(drag.y)) aim();
		frame = requestAnimationFrame(autoScroll);
	}

	// Songs, or an album, artist or playlist, dragged in from a browse view over Up Next
	// (`requirements/queue.md` §3): the gap they would drop into, as the browser reports the
	// drag, which it does steadily even when the mouse is still.
	let incoming = $state<number | null>(null);

	/** Aims a drag from a browse view, and scrolls toward an edge it is held at. */
	function dragOver(e: DragEvent) {
		if (!up || !carried.what || !scroller) return;
		e.preventDefault();
		if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
		nudge(e.clientY);
		incoming = gapAt(e.clientY);
	}

	/** Takes the line away when the drag leaves the list. */
	function dragLeave(e: DragEvent) {
		if (!scroller?.contains(e.relatedTarget as Node | null)) incoming = null;
	}

	/** Adds what was dragged in at the gap it was dropped on, in its turn after earlier edits. */
	function dragIn(e: DragEvent) {
		const { what, libraryId } = carried;
		if (!up || !what || !scroller) return;
		e.preventDefault();
		const gap = gapAt(e.clientY);
		incoming = null;
		drop();
		act(async () => addToQueue(what, libraryId, await anchorFor(gap, [])));
	}

	/** Ends a press: a drag drops where the line is; a press that did not move selects the row. */
	function dragUp() {
		const done = drag;
		dragStop();
		if (!done) return;
		if (!done.started) toggle(done.item);
		else {
			// A plain array: the page cannot post a reactive one to the worker.
			const [items, gap] = [[...done.items], done.gap];
			act(() => moveTo(items, gap));
			said = `${plural(done.items.length, 'song')} moved`;
		}
	}

	/** Drops nothing. */
	function dragStop() {
		drag = null;
		cancelAnimationFrame(frame);
	}

	/** Escape in the middle of a drag drops nothing. */
	function onkeydown(e: KeyboardEvent) {
		if (drag && e.key === 'Escape') {
			e.preventDefault();
			dragStop();
		}
	}

	/** What a row says of its track: how it is named, and that it cannot play when it cannot. */
	function title(track: TrackSummary | null | undefined) {
		return track ? track.title : track === null ? 'Unavailable track' : '';
	}
</script>

<svelte:window onkeydowncapture={onkeydown} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="scroll"
	bind:this={scroller}
	bind:clientHeight={height}
	onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
	onpointermove={dragMove}
	onpointerup={dragUp}
	onpointercancel={dragStop}
	ondragover={dragOver}
	ondragleave={dragLeave}
	ondrop={dragIn}
	class:dragging={drag?.started}
>
	{#if !queue.queueId}
		<p class="state muted">Play something to start a queue.</p>
	{:else if !up && history === 0}
		<p class="state muted">Nothing has played yet.</p>
	{:else}
		<ol class="space" style:height="{space}px" aria-label={up ? 'Up Next' : 'History'}>
			{#each rows as row (row.key)}
				{#if row.kind === 'item'}
					{@const item = row.item!}
					{@const track = shown.get(item)}
					{@const unavailable = track === null || track?.availability === 'missing'}
					<!-- Keys are handled here for the buttons inside the row. -->
					<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
					<li
						class="row"
						class:selected={selected.has(item)}
						class:dragged={drag?.started && drag.items.includes(item)}
						data-item={item}
						aria-setsize={up ? manual + upcoming : history}
						aria-posinset={row.place}
						style:top="{topOf(row.v)}px"
						onkeydown={(e) => onrowkey(e, item)}
						onkeyup={(e) => onrowkey(e, item)}
						aria-busy={track === undefined}
					>
						{#if up}
							<!-- A pointer's way to move rows; the keyboard moves them with Alt+↑/↓ from the title. -->
							<span
								class="grip"
								title="Drag to move; Alt+↑/↓ moves from the keyboard"
								aria-hidden="true"
								onpointerdown={(e) => gripDown(e, item)}
							>
								<Icon name="grip" size={18} />
							</span>
						{/if}
						<span class="thumb">
							{#if track}
								<Artwork image={track.album.image} seed={track.album.id} size={40} />
							{:else}
								<Artwork image={null} seed={item} size={40} />
							{/if}
						</span>
						<span class="text">
							<button
								type="button"
								class="title"
								class:unavailable
								disabled={unavailable}
								aria-keyshortcuts={up ? 'Alt+ArrowUp Alt+ArrowDown Delete' : 'Delete'}
								onclick={(e) => (up ? titleClick(e, item) : jump(item))}
							>
								{title(track)}
							</button>
							{#if unavailable}
								<span class="muted sub">Unavailable</span>
							{:else if track}
								<span class="muted sub">
									{track.artists.map((a) => artistName(a.name)).join(', ')}
								</span>
							{/if}
						</span>
						{#if track && !unavailable}
							<span class="muted tabular duration">{formatDuration(track.durationUs)}</span>
						{/if}
						{#if !up}
							<button
								type="button"
								class="action"
								aria-label="Play {title(track)} again"
								title="Play again"
								disabled={unavailable}
								onclick={() => jump(item)}
							>
								<Icon name="play" size={14} />
							</button>
							<button
								type="button"
								class="action"
								aria-label="Return {title(track)} to Up Next"
								title="Return to Up Next"
								onclick={() => bringBack(item)}
							>
								<Icon name="up" size={16} />
							</button>
						{/if}
						<button
							type="button"
							class="action"
							aria-label="Remove {title(track)}"
							title="Remove"
							onclick={() => remove(item)}
						>
							<Icon name="close" size={16} />
						</button>
					</li>
				{:else}
					<li class="row heading" class:hint={row.kind === 'hint'} style:top="{topOf(row.v)}px">
						{#if row.kind === 'heading'}
							<h3>
								{row.text}
								<span class="muted count tabular">
									{(row.key === 'heading-manual' ? manual : upcoming).toLocaleString()}
								</span>
							</h3>
						{:else}
							<span class="muted">Nothing queued. Queue Last adds here.</span>
						{/if}
					</li>
				{/if}
			{/each}
			{#if drag?.started || incoming !== null}
				<li
					class="drop"
					style:top="{topOf(drag?.started ? drag.gap : incoming!)}px"
					aria-hidden="true"
				></li>
			{/if}
		</ol>
	{/if}
</div>
{#if drag?.started}
	<div class="ghost" style:left="{drag.x + 14}px" style:top="{drag.y + 14}px" aria-hidden="true">
		{plural(drag.items.length, 'song')}
	</div>
{/if}
<span class="visually-hidden" role="status" aria-live="polite">{said}</span>

<style>
	.scroll {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		position: relative;
	}

	.scroll.dragging {
		user-select: none;
		cursor: grabbing;
	}

	.state {
		margin: var(--space-6) 0 0;
		text-align: center;
	}

	.space {
		position: relative;
		list-style: none;
		margin: 0;
		padding: 0;
	}

	.row {
		position: absolute;
		inset-inline: 0;
		height: 56px;
		box-sizing: border-box;
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding-inline: var(--space-1);
		border-radius: var(--radius-m);
	}

	.row.selected {
		background: var(--accent-soft);
	}

	.row.dragged {
		opacity: 0.5;
	}

	.row:hover:not(.selected, .heading) {
		background: var(--surface-hover);
	}

	.heading {
		align-items: end;
		padding-bottom: var(--space-1);
		border-bottom: 1px solid var(--border);
		border-radius: 0;
	}

	.heading.hint {
		align-items: center;
		padding: 0;
		border: 0;
		font-size: 13px;
	}

	h3 {
		margin: 0;
		font-size: 13px;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.count {
		margin-left: var(--space-1);
		font-weight: 400;
	}

	.grip {
		display: grid;
		place-items: center;
		flex-shrink: 0;
		width: 24px;
		height: 40px;
		color: var(--text-faint);
		cursor: grab;
		touch-action: none;
		user-select: none;
	}

	.grip:hover {
		color: var(--text);
	}

	.thumb {
		display: grid;
		flex-shrink: 0;
		width: 40px;
	}

	.text {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
	}

	.title {
		all: unset;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 14px;
		cursor: pointer;
	}

	.title:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
		border-radius: var(--radius-s);
	}

	.title:disabled {
		cursor: default;
	}

	.title.unavailable {
		color: var(--text-muted);
	}

	.sub {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: 12px;
	}

	.duration {
		flex-shrink: 0;
		font-size: 12px;
	}

	.action {
		display: grid;
		place-items: center;
		flex-shrink: 0;
		width: 32px;
		height: 32px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
	}

	.action:hover:not(:disabled) {
		background: var(--surface-hover);
		color: var(--text);
	}

	.action:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.drop {
		position: absolute;
		inset-inline: 0;
		height: 2px;
		margin-top: -1px;
		background: var(--accent);
		pointer-events: none;
	}

	.ghost {
		position: fixed;
		z-index: 30;
		padding: var(--space-1) var(--space-3);
		border-radius: var(--radius-m);
		background: var(--accent);
		color: var(--bg);
		font-size: 13px;
		pointer-events: none;
	}
</style>
