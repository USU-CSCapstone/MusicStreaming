<script lang="ts" module>
	/** Rows moving so the first takes index `to`, in the list as the moves before left it. */
	export type RowMove = { indexes: number[]; to: number };
</script>

<script lang="ts">
	import { tick } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { resolve } from '$app/paths';
	import type { PlayContext, TrackSummary } from '$lib/api/types';
	import { albumTitle, formatDuration, plural } from '$lib/format';
	import { player } from '$lib/player.svelte';
	import { applyOps } from '$lib/playlist-ops';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import Icon from './Icon.svelte';
	import MoveToDialog from './MoveToDialog.svelte';

	let {
		tracks,
		variant = 'default',
		sentinel,
		empty = 'No songs.',
		context = { type: 'library' },
		editable = false,
		onremove,
		onmove
	}: {
		tracks: TrackSummary[];
		/** `album`: track numbers and disc headings, no album column or artwork. */
		variant?: 'default' | 'album';
		sentinel?: (el: Element) => () => void;
		empty?: string;
		/** Where the tracks are from, which each play records. */
		context?: PlayContext;
		/**
		 * Rows can be selected, for edits the selection bar offers. A click, a long press on
		 * touch, or Space on the title then selects; a double click, the play icon, a tap with
		 * nothing selected, or Enter on the title plays.
		 */
		editable?: boolean;
		/** Removes the rows at these indexes, in order. */
		onremove?: (indexes: number[]) => void;
		/** Makes `moves` as one edit. Alt+↑/↓ and the selection bar's Move to… offer it. */
		onmove?: (moves: RowMove[]) => void;
	} = $props();

	const selected = new SvelteSet<number>();
	/** Where a Shift-click range starts: the row last clicked on its own. */
	let anchor: number | null = null;
	const uid = $props.id();
	const hintId = `selected-${uid}`;

	/** Where the selection is after a move this list made, which it keeps. */
	let kept: number[] | null = null;
	let listEl = $state<HTMLOListElement>();
	let moving = $state(false);
	/** What the last move was, for screen readers. */
	let moved = $state('');

	// Indexes shift with every change, so a selection does not outlive the list it was made
	// in, unless the change was a move made here.
	$effect.pre(() => {
		void tracks;
		const keep = kept;
		kept = null;
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

	/** Move to…: the selection, gathered in order, with its first at `to`. */
	function moveTo(to: number) {
		const indexes = [...selected].sort((a, b) => a - b);
		void move([{ indexes, to }], `${plural(indexes.length, 'song')} moved to position ${to + 1}`);
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

	// A long press selects on touch, where there is no Ctrl or Shift.
	const LONG_PRESS_MS = 500;
	let press: { timer: ReturnType<typeof setTimeout>; x: number; y: number } | null = null;
	/** The click that ends a long press is not a tap. */
	let longPressed = false;

	function pressStart(e: PointerEvent, i: number) {
		if (!editable || e.pointerType !== 'touch') return;
		longPressed = false;
		const timer = setTimeout(() => {
			press = null;
			longPressed = true;
			selected.add(i);
			anchor = i;
		}, LONG_PRESS_MS);
		press = { timer, x: e.clientX, y: e.clientY };
	}

	function pressEnd() {
		if (press) clearTimeout(press.timer);
		press = null;
	}

	/** A finger that moves is scrolling, not pressing. */
	function pressMove(e: PointerEvent) {
		if (press && Math.hypot(e.clientX - press.x, e.clientY - press.y) > 10) pressEnd();
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

	/** Ctrl/Cmd+A selects every row and Escape lets go, unless focus is somewhere they mean more. */
	function onkeydown(e: KeyboardEvent) {
		if (!editable || e.defaultPrevented) return;
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
			step(e.key === 'ArrowUp' ? -1 : 1);
		}
	}

	const isAlbum = $derived(variant === 'album');
	// Multi-disc albums are presented as discs (`requirements/albums.md` §4).
	const multiDisc = $derived(isAlbum && new Set(tracks.map((t) => t.discNumber)).size > 1);

	function play(i: number) {
		if (tracks[i].availability === 'available') player.play(tracks, i, context);
	}

	function rowClick(e: MouseEvent, i: number) {
		// Links inside the row navigate.
		if ((e.target as Element).closest('a')) return;
		if (longPressed) {
			longPressed = false;
			return;
		}
		if (editable && !(e.target as Element).closest('.num')) select(e, i);
		else play(i);
	}
</script>

<svelte:window {onkeydown} />

{#if tracks.length === 0}
	<p class="empty muted">{empty}</p>
{:else}
	<ol class="tracks" class:album={isAlbum} class:editable bind:this={listEl}>
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
				class="track"
				class:current
				class:missing
				class:selected={selected.has(i)}
				onclick={(e) => rowClick(e, i)}
				ondblclick={(e) => editable && !(e.target as Element).closest('a') && play(i)}
				onpointerdown={(e) => pressStart(e, i)}
				onpointermove={pressMove}
				onpointerup={pressEnd}
				onpointercancel={pressEnd}
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
				<span class="main">
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
					<span class="sub muted">
						{#if missing}<span class="badge">Unavailable</span>{/if}
						<ArtistLinks artists={track.artists} />
					</span>
				</span>
				{#if !isAlbum}
					<span class="album-col muted">
						<a href={resolve('/albums/[id]', { id: track.album.id })}
							>{albumTitle(track.album.title)}</a
						>
					</span>
				{/if}
				<span class="duration muted tabular">{formatDuration(track.durationUs)}</span>
			</li>
		{/each}
	</ol>
	{#if sentinel}
		<div {@attach sentinel} aria-hidden="true"></div>
	{/if}
{/if}

{#if editable}
	<span id={hintId} hidden>Selected</span>
	<span class="visually-hidden" aria-live="polite">
		{moved || (selected.size ? `${plural(selected.size, 'song')} selected` : '')}
	</span>
	{#if selected.size > 0}
		<!-- Floats over the bottom of the list as it scrolls, so the rows never jump. -->
		<div class="bar-anchor">
			<div class="bar" role="toolbar" aria-label="Selected songs">
				<span class="count"
					>{selected.size.toLocaleString()}<span class="unit"
						>{` ${selected.size === 1 ? 'song' : 'songs'}`}</span
					> selected</span
				>
				{#if onmove}
					<button type="button" class="bar-action" onclick={() => (moving = true)}>
						Move to…
					</button>
				{/if}
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
	{#if moving && selected.size > 0}
		<MoveToDialog
			count={selected.size}
			last={tracks.length - selected.size + 1}
			start={Math.min(...selected) + 1}
			onmove={moveTo}
			onclose={() => (moving = false)}
		/>
	{/if}
{/if}

<style>
	.tracks {
		list-style: none;
		margin: 0;
		padding: 0;
	}

	.track {
		display: grid;
		grid-template-columns: 32px 40px minmax(0, 3fr) minmax(0, 2fr) 56px;
		align-items: center;
		gap: var(--space-3);
		min-height: 56px;
		padding: var(--space-1) var(--space-3);
		border-radius: var(--radius-m);
		cursor: pointer;
	}

	.album .track {
		grid-template-columns: 32px minmax(0, 1fr) 56px;
		min-height: 52px;
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

	.main {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.title {
		all: unset;
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

	.sub {
		display: flex;
		gap: var(--space-2);
		min-width: 0;
		font-size: 13px;
	}

	.badge {
		flex-shrink: 0;
		padding: 0 6px;
		border-radius: 4px;
		background: var(--surface-2);
		font-size: 11px;
		line-height: 18px;
	}

	.album-col {
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
		.track {
			grid-template-columns: 40px minmax(0, 1fr) 48px;
			padding: var(--space-1) var(--space-2);
		}

		.track .num,
		.album-col {
			display: none;
		}

		.album .track {
			grid-template-columns: 28px minmax(0, 1fr) 48px;
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
