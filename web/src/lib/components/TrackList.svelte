<script lang="ts">
	import { SvelteSet } from 'svelte/reactivity';
	import { resolve } from '$app/paths';
	import type { PlayContext, TrackSummary } from '$lib/api/types';
	import { albumTitle, formatDuration, plural } from '$lib/format';
	import { player } from '$lib/player.svelte';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import Icon from './Icon.svelte';

	let {
		tracks,
		variant = 'default',
		sentinel,
		empty = 'No songs.',
		context = { type: 'library' },
		onplay,
		editable = false,
		onremove
	}: {
		tracks: TrackSummary[];
		/** `album`: track numbers and disc headings, no album column or artwork. */
		variant?: 'default' | 'album';
		sentinel?: (el: Element) => () => void;
		empty?: string;
		/** Where the tracks are from, which each play records. */
		context?: PlayContext;
		/** Called as the user starts one playing. */
		onplay?: (track: TrackSummary) => void;
		/**
		 * Rows can be selected, for edits the selection bar offers. A click then selects, and a
		 * double click, the play icon, or Enter on the title plays.
		 */
		editable?: boolean;
		/** Removes the rows at these indexes, in order. */
		onremove?: (indexes: number[]) => void;
	} = $props();

	const selected = new SvelteSet<number>();
	/** Where a Shift-click range starts: the row last clicked on its own. */
	let anchor: number | null = null;

	// Indexes shift with every change, so a selection does not outlive the list it was made in.
	$effect.pre(() => {
		void tracks;
		clear();
	});

	function clear() {
		selected.clear();
		anchor = null;
	}

	function toggle(i: number) {
		if (!selected.delete(i)) selected.add(i);
		anchor = i;
	}

	function select(e: MouseEvent, i: number) {
		const adding = e.metaKey || e.ctrlKey;
		// On touch, a tap plays until something is selected, and then picks rows like the boxes.
		const touch = (e as PointerEvent).pointerType === 'touch';
		if (e.shiftKey && anchor !== null) {
			if (!adding) selected.clear();
			for (let k = Math.min(anchor, i); k <= Math.max(anchor, i); k++) selected.add(k);
		} else if (adding || (touch && selected.size > 0)) {
			toggle(i);
		} else if (touch) {
			play(i);
		} else {
			selected.clear();
			toggle(i);
		}
	}

	function remove() {
		onremove?.([...selected].sort((a, b) => a - b));
		clear();
	}

	/** Ctrl/Cmd+A selects every row and Escape lets go, unless focus is somewhere they mean more. */
	function onkeydown(e: KeyboardEvent) {
		if (!editable || e.defaultPrevented) return;
		if (
			(e.target as Element).closest(
				'input:not([type="checkbox"]), textarea, select, [contenteditable], dialog'
			)
		) {
			return;
		}
		if (e.key === 'a' && (e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey) {
			e.preventDefault();
			tracks.forEach((_, i) => selected.add(i));
		} else if (e.key === 'Escape' && selected.size > 0) {
			clear();
		}
	}

	const isAlbum = $derived(variant === 'album');
	// Multi-disc albums are presented as discs (`requirements/albums.md` §4).
	const multiDisc = $derived(isAlbum && new Set(tracks.map((t) => t.discNumber)).size > 1);

	function play(i: number) {
		if (tracks[i].availability !== 'available') return;
		player.play(tracks, i, context);
		onplay?.(tracks[i]);
	}

	function rowClick(e: MouseEvent, i: number) {
		const target = e.target as Element;
		// Links inside the row navigate, and its box selects on its own.
		if (target.closest('a, input')) return;
		if (editable && !target.closest('.num')) select(e, i);
		else play(i);
	}
</script>

<svelte:window {onkeydown} />

{#if tracks.length === 0}
	<p class="empty muted">{empty}</p>
{:else}
	<ol class="tracks" class:album={isAlbum} class:editable>
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
				ondblclick={(e) => editable && !(e.target as Element).closest('a, input') && play(i)}
			>
				{#if editable}
					<span class="check">
						<input
							type="checkbox"
							aria-label="Select {track.title}"
							checked={selected.has(i)}
							onchange={() => toggle(i)}
						/>
					</span>
				{/if}
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
					<!-- A pointer's click on the title selects when rows do; Enter (detail 0) plays. -->
					<button
						type="button"
						class="title"
						disabled={missing}
						onclick={(e) => {
							if (!editable || e.detail === 0) play(i);
						}}
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
	<span class="visually-hidden" aria-live="polite">
		{selected.size ? `${plural(selected.size, 'song')} selected` : ''}
	</span>
	{#if selected.size > 0}
		<!-- Floats over the bottom of the list as it scrolls, so the rows never jump. -->
		<div class="bar-anchor">
			<div class="bar" role="toolbar" aria-label="Selected songs">
				<span class="count">{plural(selected.size, 'song')} selected</span>
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
		grid-template-columns: 28px 32px 40px minmax(0, 3fr) minmax(0, 2fr) 56px;
		/* Shift-click selects rows, not text. */
		user-select: none;
	}

	.track.selected,
	.track.selected:hover {
		background: var(--accent-soft);
	}

	.check {
		display: grid;
		place-items: center;
	}

	.check input {
		width: 16px;
		height: 16px;
		margin: 0;
		accent-color: var(--accent);
		cursor: pointer;
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
		margin-right: var(--space-2);
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

		.editable .track {
			grid-template-columns: 28px 40px minmax(0, 1fr) 48px;
		}
	}
</style>
