<script lang="ts" module>
	/** Rows moving so the first takes index `to`, in the list as the moves before left it. */
	export type RowMove = { indexes: number[]; to: number };
</script>

<script lang="ts">
	import { tick } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import type { PlayContext, QueueQuery, TrackSummary } from '$lib/api/types';
	import { albumTitle, formatDate, formatDuration, plural, releaseYear } from '$lib/format';
	import { canDrag, drop, edgeStep, lift } from '$lib/dragging';
	import { player } from '$lib/player.svelte';
	import { songs } from '$lib/adding.svelte';
	import { applyOps } from '$lib/playlist-ops';
	import {
		ALBUM_FIELDS,
		FIELDS,
		nextSort,
		PLAYLIST_FIELDS,
		type SortField,
		type TrackSort
	} from '$lib/sorting';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import AddToPlaylistMenu from './AddToPlaylistMenu.svelte';
	import Icon from './Icon.svelte';
	import SortMenu from './SortMenu.svelte';

	let {
		tracks,
		variant = 'default',
		added,
		sort = null,
		onsort,
		sentinel,
		empty = 'No songs.',
		context = { type: 'library' },
		query,
		onplay,
		editable = false,
		onremove,
		onmove
	}: {
		tracks: TrackSummary[];
		/**
		 * `album`: track numbers and, unsorted, disc headings; no album or year column, and no
		 * artwork.
		 */
		variant?: 'default' | 'album';
		/**
		 * When each row was added to the playlist, by index, for a Date added column. Only
		 * playlists have one (`requirements/playlists.md` §10).
		 */
		added?: readonly string[];
		/** The order `tracks` are in, which the header marks. */
		sort?: TrackSort;
		/**
		 * Asks for the list in another order, from a click on a column's heading
		 * (`requirements/conventions.md` §4.1). Without it the headings only label the columns.
		 */
		onsort?: (sort: TrackSort) => void;
		sentinel?: (el: Element) => () => void;
		empty?: string;
		/** Where the tracks are from, which each play records. */
		context?: PlayContext;
		/**
		 * The list `tracks` pages through, when it is only part of it: a play starts a queue from
		 * the list itself, which the server reads to its end, rather than from the tracks held.
		 */
		query?: QueueQuery;
		/** Called as the user starts one playing. */
		onplay?: (track: TrackSummary) => void;
		/**
		 * Rows can be selected, for edits the selection bar offers. A click, a long press on
		 * touch, or Space on the title then selects; a double click, the play icon, a tap with
		 * nothing selected, or Enter on the title plays.
		 */
		editable?: boolean;
		/** Removes the rows at these indexes, in order. */
		onremove?: (indexes: number[]) => void;
		/**
		 * Makes `moves` as one edit, by dragging or Alt+↑/↓. Not while sorted: a place between
		 * two rows of a sort is no place in the list's own order (`requirements/playlists.md` §4).
		 */
		onmove?: (moves: RowMove[]) => void;
	} = $props();

	const movable = $derived(onmove !== undefined && sort === null);

	const selected = new SvelteSet<number>();
	/** Where a Shift-click range starts: the row last clicked on its own. */
	let anchor: number | null = null;
	const uid = $props.id();
	const hintId = `selected-${uid}`;

	// Dragging rows, with pointer events so touch works as a mouse does. A row dragged is the
	// selection if it is selected, and becomes the selection if not.
	type Drag = {
		indexes: number[];
		/** Where the pointer is. */
		x: number;
		y: number;
		/** Once it has moved far enough to be a drag rather than a press. */
		started: boolean;
		/** The gap the rows would drop into, counting from the top, and where to draw it. */
		gap: number | null;
		line: number;
	};
	let drag = $state<Drag | null>(null);
	let dragFrom = { x: 0, y: 0 };
	let scroller: Element | null = null;
	let frame = 0;
	/** Where the pointer is captured once a drag starts, so it can leave the row. */
	let dragPointer: { row: Element; id: number } | null = null;

	/** Where the selection is after a move this list made, which it keeps. */
	let kept: number[] | null = null;
	let listEl = $state<HTMLOListElement>();
	/** What the last move was, for screen readers. */
	let moved = $state('');

	// Indexes shift with every change, so a selection does not outlive the list it was made
	// in, unless the change was a move made here.
	$effect.pre(() => {
		void tracks;
		const keep = kept;
		kept = null;
		dragStop();
		clear();
		for (const i of keep ?? []) selected.add(i);
		anchor = keep?.[0] ?? null;
	});

	function clear() {
		selected.clear();
		anchor = null;
		moved = '';
	}

	/**
	 * Alt+↑/↓: each selected row steps one place, as lines do in an editor, so a scattered
	 * selection stays scattered. A row at the edge, or against one that cannot move, stays.
	 */
	function step(by: -1 | 1) {
		const order = [...selected].sort((a, b) => (a - b) * -by);
		let blocked = by === -1 ? -1 : tracks.length;
		const moves: RowMove[] = [];
		for (const i of order) {
			if (i + by === blocked) blocked = i;
			else moves.push({ indexes: [i], to: i + by });
		}
		if (moves.length)
			void move(moves, `${plural(selected.size, 'song')} moved ${by < 0 ? 'up' : 'down'}`);
	}

	/** Makes `moves`, keeping the moved rows selected and focus on the row it was on. */
	async function move(moves: RowMove[], said: string) {
		// Where each row ends up: the moves, made on the indexes themselves.
		const ops = moves.map((m) => ({ op: 'move' as const, positions: m.indexes, at: m.to }));
		const order = applyOps(
			tracks.map((_, i) => i),
			ops,
			() => []
		);
		const titles = () => [...(listEl?.querySelectorAll<HTMLElement>('.title') ?? [])];
		const focused = titles().findIndex((t) => t === document.activeElement);
		kept = order.flatMap((was, now) => (selected.has(was) ? [now] : []));
		try {
			onmove?.(moves);
		} catch (e) {
			kept = null;
			throw e;
		}
		await tick();
		moved = said;
		if (focused >= 0) titles()[order.indexOf(focused)]?.focus();
	}

	function toggle(i: number) {
		if (!selected.delete(i)) selected.add(i);
		anchor = i;
		moved = '';
	}

	/**
	 * Shift extends from the anchor, and Ctrl/Cmd adds or takes away one. Otherwise a selected
	 * row is let go, and any other becomes the selection. On touch, a tap plays until something
	 * is selected, and then adds or takes away a row.
	 */
	function select(e: MouseEvent | KeyboardEvent, i: number) {
		const adding = e.metaKey || e.ctrlKey;
		const touch = (e as PointerEvent).pointerType === 'touch';
		if (e.shiftKey && anchor !== null) {
			moved = '';
			if (!adding) selected.clear();
			for (let k = Math.min(anchor, i); k <= Math.max(anchor, i); k++) selected.add(k);
		} else if (adding || selected.has(i) || (touch && selected.size > 0)) {
			toggle(i);
		} else if (touch) {
			play(i);
		} else {
			selected.clear();
			toggle(i);
		}
	}

	// A row is dragged from anywhere on it. With a mouse, a press that moves a few pixels is a
	// drag, and one that does not is a click. On touch, where a moving finger scrolls, a long
	// press selects the row and lifts the selection, and the finger then drags it.
	const LONG_PRESS_MS = 500;
	let press: { timer: ReturnType<typeof setTimeout>; x: number; y: number } | null = null;
	/** The click that ends a long press, or a drag, is not a click on the row. */
	let longPressed = false;
	let dragged = false;

	function pointerDown(e: PointerEvent, i: number) {
		if (!editable || e.button !== 0 || (e.target as Element).closest('.more')) return;
		longPressed = false;
		dragged = false;
		const row = e.currentTarget as Element;
		const at = { x: e.clientX, y: e.clientY };
		if (e.pointerType === 'touch') {
			const timer = setTimeout(() => {
				press = null;
				longPressed = true;
				if (!selected.has(i)) {
					selected.add(i);
					anchor = i;
				}
				if (movable) arm(i, row, e.pointerId, at);
			}, LONG_PRESS_MS);
			press = { timer, ...at };
		} else if (movable && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
			arm(i, row, e.pointerId, at);
		}
	}

	/** Ready to drag row `i`, or the selection if it is in it, once the pointer moves. */
	function arm(i: number, row: Element, id: number, at: { x: number; y: number }) {
		const indexes = selected.has(i) ? [...selected].sort((a, b) => a - b) : [i];
		drag = { indexes, ...at, started: false, gap: null, line: 0 };
		dragFrom = at;
		dragPointer = { row, id };
	}

	function pointerMove(e: PointerEvent) {
		// A finger that moves before the long press is scrolling.
		if (press && Math.hypot(e.clientX - press.x, e.clientY - press.y) > 10) pressEnd();
		if (!drag) return;
		drag.x = e.clientX;
		drag.y = e.clientY;
		if (!drag.started) {
			if (Math.hypot(e.clientX - dragFrom.x, e.clientY - dragFrom.y) < 4) return;
			drag.started = true;
			try {
				dragPointer?.row.setPointerCapture(dragPointer.id);
			} catch {
				// The pointer is gone already; the drag ends with it.
			}
			if (!selected.has(drag.indexes[0])) {
				clear();
				selected.add(drag.indexes[0]);
				anchor = drag.indexes[0];
			}
			scroller = scrollParent(listEl);
			frame = requestAnimationFrame(autoScroll);
		}
		aim();
	}

	function pointerUp() {
		pressEnd();
		if (drag?.started) dragged = true;
		dragEnd();
	}

	function pointerCancel() {
		pressEnd();
		dragStop();
	}

	function pressEnd() {
		if (press) clearTimeout(press.timer);
		press = null;
	}

	/** Once a long press has lifted rows, the finger drags them rather than scrolling the page. */
	function holdTouch(el: HTMLElement) {
		const hold = (e: TouchEvent) => drag && e.preventDefault();
		el.addEventListener('touchmove', hold, { passive: false });
		return () => el.removeEventListener('touchmove', hold);
	}

	/** Space on a title selects, as Enter plays. */
	function titleKey(e: KeyboardEvent, i: number) {
		if (!editable || e.key !== ' ') return;
		// The browser would click the button on Space too.
		e.preventDefault();
		if (e.type === 'keydown') select(e, i);
	}

	function remove() {
		onremove?.([...selected].sort((a, b) => a - b));
		clear();
	}

	function dragEnd() {
		const done = drag;
		dragStop();
		if (!done?.started || done.gap === null) return;
		const { indexes, gap } = done;
		const to = gap - indexes.filter((k) => k < gap).length;
		// Dropped where they were, together: nothing moved.
		if (indexes.every((k, j) => k === to + j)) return;
		void move([{ indexes, to }], `${plural(indexes.length, 'song')} moved to position ${to + 1}`);
	}

	function dragStop() {
		drag = null;
		dragPointer = null;
		cancelAnimationFrame(frame);
	}

	/** The gap nearest the pointer: before the first row whose middle is below it. */
	function aim() {
		if (!drag || !listEl) return;
		const rows = [...listEl.querySelectorAll<HTMLElement>('.track')];
		const k = rows.findIndex((r) => {
			const box = r.getBoundingClientRect();
			return drag!.y < box.top + box.height / 2;
		});
		const last = rows.at(-1)!;
		drag.gap = k === -1 ? rows.length : k;
		drag.line = k === -1 ? last.offsetTop + last.offsetHeight : rows[k].offsetTop;
	}

	/** Near the top or bottom of what scrolls, scrolls toward it, faster the closer it gets. */
	function autoScroll() {
		if (!drag?.started) return;
		const box = scroller?.getBoundingClientRect() ?? { top: 0, bottom: innerHeight };
		const step = edgeStep(drag.y, box);
		if (step) {
			(scroller ?? document.scrollingElement!).scrollTop += step;
			aim();
		}
		frame = requestAnimationFrame(autoScroll);
	}

	/** The nearest ancestor that scrolls, or null for the page itself. */
	function scrollParent(el: Element | undefined): Element | null {
		for (let p = el?.parentElement; p; p = p.parentElement) {
			if (/auto|scroll/.test(getComputedStyle(p).overflowY)) return p;
		}
		return null;
	}

	/** Ctrl/Cmd+A selects every row and Escape lets go, unless focus is somewhere they mean more. */
	function onkeydown(e: KeyboardEvent) {
		if (!editable || e.defaultPrevented) return;
		if (drag && e.key === 'Escape') {
			// Escape drops nothing.
			e.preventDefault();
			dragStop();
			return;
		}
		if ((e.target as Element).closest('input, textarea, select, [contenteditable], dialog')) {
			return;
		}
		if (e.key === 'a' && (e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey) {
			e.preventDefault();
			moved = '';
			tracks.forEach((_, i) => selected.add(i));
		} else if (e.key === 'Escape' && selected.size > 0) {
			clear();
		} else if (
			(e.key === 'ArrowUp' || e.key === 'ArrowDown') &&
			e.altKey &&
			onmove &&
			selected.size > 0
		) {
			e.preventDefault();
			if (movable) step(e.key === 'ArrowUp' ? -1 : 1);
			else moved = 'The list is sorted, so songs can’t be moved';
		}
	}

	const isAlbum = $derived(variant === 'album');
	// Multi-disc albums are presented as discs (`requirements/albums.md` §4). Sorted, the
	// tracks are one run: a disc's heading would stand over songs from either disc.
	const multiDisc = $derived(
		isAlbum && sort === null && new Set(tracks.map((t) => t.discNumber)).size > 1
	);

	const labels: Record<SortField, string> = {
		title: 'Title',
		artist: 'Artist',
		album: 'Album',
		year: 'Year',
		added: 'Date added',
		duration: 'Duration'
	};
	/** The columns this list shows, each of which it sorts by. */
	const columns: readonly SortField[] = $derived(
		isAlbum ? ALBUM_FIELDS : added ? PLAYLIST_FIELDS : FIELDS
	);
	/** What the last click on a heading did, for screen readers. */
	let sorted = $state('');

	function sortBy(field: SortField) {
		const next = nextSort(sort, field);
		onsort?.(next);
		sorted = next
			? `Sorted by ${labels[field].toLowerCase()}, ${next.order === 'asc' ? 'ascending' : 'descending'}`
			: 'Unsorted';
	}

	/** A heading's name says how the list is sorted by it, if it is. */
	function sortLabel(field: SortField): string {
		if (sort?.field !== field) return labels[field];
		return `${labels[field]}, sorted ${sort.order === 'asc' ? 'ascending' : 'descending'}`;
	}

	function play(i: number) {
		if (tracks[i].availability !== 'available') return;
		player.play(tracks, i, context, query);
		onplay?.(tracks[i]);
	}

	/** Songs on their way to a playlist, and the row's ⋯ or the selection bar that opened it. */
	let adding = $state<{
		tracks: TrackSummary[];
		anchor: HTMLElement;
		from: number | 'bar';
	} | null>(null);

	/** The row's ⋯ adds the row, or the selection if the row is in it, as a second click closes. */
	function more(e: MouseEvent, i: number) {
		if (adding?.from === i) return closeAdding();
		const indexes = selected.has(i) ? [...selected].sort((a, b) => a - b) : [i];
		adding = {
			tracks: indexes.map((k) => tracks[k]),
			anchor: e.currentTarget as HTMLElement,
			from: i
		};
	}

	/**
	 * A mouse drags the row, or the selection if the row is in it, toward the queue panel. Not
	 * where rows are moved by dragging, which is the pointer's own.
	 */
	function dragRow(e: DragEvent, i: number) {
		if (onmove) return;
		const indexes = selected.has(i) ? [...selected].sort((a, b) => a - b) : [i];
		lift(e, songs(indexes.map((k) => tracks[k])), page.data.library?.id ?? '');
	}

	function addSelected(e: MouseEvent) {
		if (adding?.from === 'bar') return closeAdding();
		const indexes = [...selected].sort((a, b) => a - b);
		adding = {
			tracks: indexes.map((k) => tracks[k]),
			anchor: e.currentTarget as HTMLElement,
			from: 'bar'
		};
	}

	/** Focus goes back to what opened the menu. */
	function closeAdding() {
		const anchor = adding?.anchor;
		adding = null;
		if (anchor?.isConnected) anchor.focus();
	}

	function rowClick(e: MouseEvent, i: number) {
		// Links inside the row navigate, and its ⋯ opens its own menu.
		if ((e.target as Element).closest('a, .more')) return;
		if (longPressed || dragged) {
			longPressed = dragged = false;
			return;
		}
		if (editable && !(e.target as Element).closest('.num')) select(e, i);
		// The title button has played it already, and a second start would be a second queue.
		else if (!(e.target as Element).closest('.title')) play(i);
	}
</script>

<svelte:window {onkeydown} />

{#if tracks.length === 0}
	<p class="empty muted">{empty}</p>
{:else}
	<!-- The list's own width, not the window's, decides its columns: the sidebar and the
	     page's padding take some of the window. -->
	<div class="list" class:album={isAlbum} class:dated={added}>
		{#if onsort}
			<!-- Where the headings don't fit, the same sorts are one button away, which stays in
			     view as the header does. -->
			<div class="sorter">
				<SortMenu fields={columns} {labels} {sort} onchoose={sortBy} />
			</div>
		{/if}
		<!-- Stays in view as the list scrolls, so changing the sort never means going back to
		     the top (`requirements/conventions.md` §4.1). -->
		<div class="row head" role="group" aria-label="Sort songs">
			<span class="heading" style:grid-area="num" aria-hidden="true">#</span>
			{#each columns as field (field)}
				{@const order = sort?.field === field ? sort.order : null}
				<span class="heading" class:end={field === 'duration'} style:grid-area={field}>
					{#if onsort}
						<button
							type="button"
							class="sort"
							class:active={order}
							aria-label={sortLabel(field)}
							title="Sort by {labels[field].toLowerCase()}"
							onclick={() => sortBy(field)}
						>
							{#if field === 'duration'}<Icon name="clock" size={15} />{:else}{labels[field]}{/if}
							{#if order}<Icon name={order === 'asc' ? 'up' : 'down'} size={13} />{/if}
						</button>
					{:else if field === 'duration'}
						<span role="img" aria-label="Duration"><Icon name="clock" size={15} /></span>
					{:else}
						{labels[field]}
					{/if}
				</span>
			{/each}
		</div>
		<span class="visually-hidden" aria-live="polite">{sorted}</span>
		<ol
			class="tracks"
			class:editable
			class:reorderable={editable && movable}
			class:dragging={drag?.started}
			bind:this={listEl}
			{@attach holdTouch}
			ondragstart={(e) => onmove && e.preventDefault()}
		>
			<!-- Keyed by position: a playlist may hold the same track twice. -->
			{#each tracks as track, i (i)}
				{#if multiDisc && (i === 0 || tracks[i - 1].discNumber !== track.discNumber)}
					<li class="disc">Disc {track.discNumber ?? '—'}</li>
				{/if}
				{@const current = player.current?.id === track.id}
				{@const missing = track.availability === 'missing'}
				<!-- Clicking anywhere on the row is a pointer convenience; the title button is the
			     keyboard path. -->
				<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
				<li
					class="row track"
					class:current
					class:missing
					class:selected={selected.has(i)}
					class:dragged={drag?.started && drag.indexes.includes(i)}
					onclick={(e) => rowClick(e, i)}
					ondblclick={(e) => editable && !(e.target as Element).closest('a, .more') && play(i)}
					onpointerdown={(e) => pointerDown(e, i)}
					onpointermove={pointerMove}
					onpointerup={pointerUp}
					onpointercancel={pointerCancel}
					draggable={canDrag && !onmove}
					ondragstart={(e) => dragRow(e, i)}
					ondragend={drop}
					oncontextmenu={(e) => (press || longPressed) && e.preventDefault()}
				>
					<span class="num tabular">
						{#if current && player.playing}
							<span class="eq" aria-label="Playing"><i></i><i></i><i></i></span>
						{:else}
							<span class="n">{isAlbum ? (track.trackNumber ?? '–') : i + 1}</span>
							<span class="play-icon"><Icon name="play" size={14} /></span>
						{/if}
					</span>
					{#if !isAlbum}
						<span class="thumb">
							<Artwork image={track.album.image} seed={track.album.id} size={40} />
						</span>
					{/if}
					<span class="title-col">
						<!-- A pointer's click on the title selects when rows do; Enter (detail 0) plays.
					     A missing track can still be selected, to remove it. -->
						<button
							type="button"
							class="title"
							disabled={missing && !editable}
							aria-describedby={selected.has(i) ? hintId : undefined}
							onclick={(e) => {
								if (!editable || e.detail === 0) play(i);
							}}
							onkeydown={(e) => titleKey(e, i)}
							onkeyup={(e) => titleKey(e, i)}
						>
							{track.title}
						</button>
						{#if missing}<span class="badge">Unavailable</span>{/if}
					</span>
					<!-- Under the title where there is no room for its own column. -->
					<span class="artist-col muted"><ArtistLinks artists={track.artists} /></span>
					{#if !isAlbum}
						<span class="album-col muted">
							<a href={resolve('/albums/[id]', { id: track.album.id })}
								>{albumTitle(track.album.title)}</a
							>
						</span>
						<span class="year muted tabular">{releaseYear(track.releaseDate) ?? ''}</span>
					{/if}
					{#if added}
						<span class="added muted tabular">{added[i] ? formatDate(added[i]) : ''}</span>
					{/if}
					<span class="duration muted tabular">{formatDuration(track.durationUs)}</span>
					<button
						type="button"
						class="more"
						class:open={adding?.from === i}
						aria-expanded={adding?.from === i}
						aria-label="Actions for {track.title}"
						title="More actions"
						aria-haspopup="menu"
						onclick={(e) => more(e, i)}
					>
						<Icon name="more" size={18} />
					</button>
				</li>
			{/each}
			{#if drag?.started && drag.gap !== null}
				<li class="drop" style:top="{drag.line}px" aria-hidden="true"></li>
			{/if}
		</ol>
	</div>
	{#if drag?.started}
		<div class="ghost" style:left="{drag.x + 14}px" style:top="{drag.y + 14}px" aria-hidden="true">
			{plural(drag.indexes.length, 'song')}
		</div>
	{/if}
	{#if sentinel}
		<div {@attach sentinel} aria-hidden="true"></div>
	{/if}
{/if}

{#if adding}
	<AddToPlaylistMenu what={songs(adding.tracks)} anchor={adding.anchor} onclose={closeAdding} />
{/if}

{#if editable}
	<span id={hintId} hidden>Selected</span>
	<span class="visually-hidden" aria-live="polite">
		{moved || (selected.size ? `${plural(selected.size, 'song')} selected` : '')}
	</span>
	{#if selected.size > 0 && !drag?.started}
		<!-- Floats over the bottom of the list as it scrolls, so the rows never jump. It steps
		     aside during a drag, so as not to hide where the rows would land. -->
		<div class="bar-anchor">
			<div class="bar" role="toolbar" aria-label="Selected songs">
				<span class="count"
					>{selected.size.toLocaleString()}<span class="unit"
						>{` ${selected.size === 1 ? 'song' : 'songs'}`}</span
					> selected</span
				>
				<button
					type="button"
					class="bar-action"
					aria-haspopup="menu"
					aria-expanded={adding?.from === 'bar'}
					aria-label="Add to queue or playlist"
					onclick={addSelected}
				>
					<Icon name="plus" size={16} />
					<span>Add <span class="unit">to…</span></span>
				</button>
				{#if onremove}
					<button type="button" class="bar-action" onclick={remove}>
						<Icon name="trash" size={16} /> Remove
					</button>
				{/if}
				<button type="button" class="bar-close" aria-label="Clear selection" onclick={clear}>
					<Icon name="close" size={16} />
				</button>
			</div>
		</div>
	{/if}
{/if}

<style>
	.list {
		container: tracks / inline-size;
	}

	.tracks {
		list-style: none;
		margin: 0;
		padding: 0;
	}

	/* Rows and the header share their columns. Each cell has an area, so the artists move
	   between the title's second line and a column of their own without being drawn twice. */
	.row {
		display: grid;
		grid-template-columns: 32px 40px minmax(0, 3fr) minmax(0, 2fr) 56px 36px;
		grid-template-areas:
			'num thumb title album duration more'
			'num thumb artist album duration more';
		align-items: center;
		column-gap: var(--space-3);
		padding-inline: var(--space-3);
	}

	.album .row {
		grid-template-columns: 32px minmax(0, 1fr) 56px 36px;
		grid-template-areas:
			'num title duration more'
			'num artist duration more';
	}

	.track {
		min-height: 56px;
		padding-block: var(--space-1);
		border-radius: var(--radius-m);
		cursor: pointer;
	}

	.album .track {
		min-height: 52px;
	}

	/* Only where every column has its heading; narrower lists sort from a menu instead. */
	.head {
		display: none;
		position: sticky;
		/* Flush with the top of what scrolls: at 0 it would stop below the page's top padding,
		   with rows showing through above it. */
		top: calc(-1 * var(--page-top, 0px));
		z-index: 3;
		margin-bottom: var(--space-2);
		padding-block: var(--space-2);
		border-bottom: 1px solid var(--border);
		background: var(--bg);
		font-size: 13px;
		color: var(--text-muted);
	}

	.heading {
		display: flex;
		min-width: 0;
	}

	/* Stays in view as the header does. */
	.sorter {
		display: flex;
		justify-content: flex-end;
		position: sticky;
		top: calc(-1 * var(--page-top, 0px));
		z-index: 3;
		margin-bottom: var(--space-2);
		padding-block: var(--space-2);
		background: var(--bg);
	}

	.heading.end {
		justify-content: flex-end;
	}

	.sort {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		min-width: 0;
		padding: 0;
		border: 0;
		border-radius: 4px;
		background: none;
		color: inherit;
		font: inherit;
		cursor: pointer;
	}

	.sort:hover,
	.sort.active {
		color: var(--text);
	}

	.sort:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	.num {
		grid-area: num;
	}

	.thumb {
		grid-area: thumb;
	}

	.title-col {
		grid-area: title;
		align-self: end;
	}

	.artist-col {
		grid-area: artist;
		align-self: start;
		font-size: 13px;
	}

	.album-col {
		grid-area: album;
	}

	.year {
		grid-area: year;
	}

	.added {
		grid-area: added;
	}

	.duration {
		grid-area: duration;
	}

	.more {
		grid-area: more;
	}

	.year,
	.added {
		display: none;
	}

	/* Wide enough for every column: the artists get their own, and year and date added show. */
	@container tracks (min-width: 880px) {
		.row {
			grid-template-columns:
				32px 40px minmax(0, 3fr) minmax(0, 2fr) minmax(0, 2fr)
				48px 56px 36px;
			grid-template-areas: 'num thumb title artist album year duration more';
		}

		.dated .row {
			grid-template-columns:
				32px 40px minmax(0, 3fr) minmax(0, 2fr) minmax(0, 2fr)
				48px 104px 56px 36px;
			grid-template-areas: 'num thumb title artist album year added duration more';
		}

		.album .row {
			grid-template-columns: 32px minmax(0, 3fr) minmax(0, 2fr) 56px 36px;
			grid-template-areas: 'num title artist duration more';
		}

		.title-col,
		.artist-col {
			align-self: center;
		}

		.artist-col {
			font-size: inherit;
		}

		.year,
		.added {
			display: block;
		}

		.head {
			display: grid;
		}

		.sorter {
			display: none;
		}
	}

	.more {
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
	}

	.more:hover,
	.more.open {
		background: var(--surface-hover);
		color: var(--text);
	}

	/* With a mouse, ⋯ shows on the row being pointed at, focused, or selected; on touch, always. */
	@media (hover: hover) {
		.more:not(.open) {
			opacity: 0;
		}

		.track:hover .more,
		.track:focus-within .more,
		.track.selected .more {
			opacity: 1;
		}
	}

	.track:hover {
		background: var(--surface-hover);
	}

	/* Room under the last row for the selection bar, which never covers a row it can scroll off. */
	.tracks.editable {
		padding-bottom: 64px;
	}

	.editable .track {
		/* Shift-click and a long press select rows, not text. */
		user-select: none;
		-webkit-touch-callout: none;
	}

	.reorderable {
		position: relative;
	}

	.dragging,
	.dragging .track {
		cursor: grabbing;
	}

	.track.dragged {
		opacity: 0.4;
	}

	.drop {
		position: absolute;
		left: 0;
		right: 0;
		height: 2px;
		margin-top: -1px;
		border-radius: 1px;
		background: var(--accent);
		pointer-events: none;
	}

	.ghost {
		position: fixed;
		z-index: 30;
		padding: var(--space-1) var(--space-3);
		border-radius: var(--radius-m);
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		pointer-events: none;
	}

	.track.selected,
	.track.selected:hover {
		background: var(--accent-soft);
	}

	.bar-anchor {
		position: sticky;
		bottom: var(--space-4);
		height: 0;
		z-index: 5;
	}

	.bar {
		position: absolute;
		bottom: 0;
		left: 50%;
		transform: translateX(-50%);
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-1) var(--space-1) var(--space-1) var(--space-4);
		border: 1px solid var(--border);
		border-radius: 24px;
		background: var(--surface);
		box-shadow: var(--shadow);
		white-space: nowrap;
	}

	.count {
		min-width: 0;
		margin-right: var(--space-2);
		overflow: hidden;
		text-overflow: ellipsis;
		font-weight: 500;
	}

	.bar button {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		height: 36px;
		border: 0;
		border-radius: 18px;
		background: transparent;
		color: var(--text);
		cursor: pointer;
	}

	.bar-action {
		padding: 0 var(--space-3);
		font-weight: 500;
	}

	.bar .bar-close {
		justify-content: center;
		width: 36px;
		color: var(--text-muted);
	}

	.bar button:hover {
		background: var(--surface-hover);
	}

	.track.current .title,
	.track.current .num {
		color: var(--accent);
	}

	.track.missing {
		cursor: default;
		opacity: 0.55;
	}

	.num {
		display: grid;
		place-items: center;
		color: var(--text-muted);
	}

	.play-icon {
		display: none;
	}

	.track:not(.missing):hover .n {
		display: none;
	}

	.track:not(.missing):hover .play-icon {
		display: grid;
		color: var(--text);
	}

	.title-col {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
	}

	.title {
		all: unset;
		min-width: 0;
		font-weight: 500;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		cursor: inherit;
	}

	.title:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
		border-radius: 2px;
	}

	.artist-col {
		display: flex;
		min-width: 0;
	}

	.badge {
		flex-shrink: 0;
		padding: 0 6px;
		border-radius: 4px;
		background: var(--surface-2);
		font-size: 11px;
		line-height: 18px;
	}

	.album-col,
	.added {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.duration {
		text-align: right;
	}

	.disc {
		padding: var(--space-4) var(--space-3) var(--space-2);
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: var(--text-muted);
	}

	.disc:first-child {
		padding-top: 0;
	}

	.eq {
		display: flex;
		align-items: flex-end;
		gap: 2px;
		height: 14px;
	}

	.eq i {
		width: 3px;
		background: var(--accent);
		border-radius: 1px;
		animation: eq 0.9s ease-in-out infinite;
	}

	.eq i:nth-child(2) {
		animation-delay: -0.3s;
	}

	.eq i:nth-child(3) {
		animation-delay: -0.6s;
	}

	@keyframes eq {
		0%,
		100% {
			height: 30%;
		}
		50% {
			height: 100%;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.eq i {
			animation: none;
			height: 70%;
		}
	}

	.empty {
		padding: var(--space-6) 0;
	}

	@media (max-width: 767px) {
		.row {
			grid-template-columns: 40px minmax(0, 1fr) 48px 36px;
			grid-template-areas:
				'thumb title duration more'
				'thumb artist duration more';
			padding-inline: var(--space-2);
		}

		.track .num,
		.album-col {
			display: none;
		}

		.album .row {
			grid-template-columns: 28px minmax(0, 1fr) 48px 36px;
			grid-template-areas:
				'num title duration more'
				'num artist duration more';
		}

		.album .track .num {
			display: grid;
		}

		/* The bar fits the screen: "3 selected", and Remove keeps its word but not its icon. */
		.unit {
			display: none;
		}

		.bar {
			max-width: calc(100vw - 2 * var(--space-3));
		}

		.bar-action :global(svg) {
			display: none;
		}
	}
</style>
