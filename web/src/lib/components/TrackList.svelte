<script lang="ts">
	import { resolve } from '$app/paths';
	import type { PlayContext, TrackSummary } from '$lib/api/types';
	import { albumTitle, formatDuration } from '$lib/format';
	import { player } from '$lib/player.svelte';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import Icon from './Icon.svelte';

	let {
		tracks,
		variant = 'default',
		sentinel,
		empty = 'No songs.',
		context = { type: 'library' }
	}: {
		tracks: TrackSummary[];
		/** `album`: track numbers and disc headings, no album column or artwork. */
		variant?: 'default' | 'album';
		sentinel?: (el: Element) => () => void;
		empty?: string;
		/** Where the tracks are from, which each play records. */
		context?: PlayContext;
	} = $props();

	const isAlbum = $derived(variant === 'album');
	// Multi-disc albums are presented as discs (`requirements/albums.md` §4).
	const multiDisc = $derived(isAlbum && new Set(tracks.map((t) => t.discNumber)).size > 1);

	function play(i: number) {
		if (tracks[i].availability === 'available') player.play(tracks, i, context);
	}

	function rowClick(e: MouseEvent, i: number) {
		// Links inside the row navigate rather than play.
		if ((e.target as Element).closest('a')) return;
		play(i);
	}
</script>

{#if tracks.length === 0}
	<p class="empty muted">{empty}</p>
{:else}
	<ol class="tracks" class:album={isAlbum}>
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
			<li class="track" class:current class:missing onclick={(e) => rowClick(e, i)}>
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
					<button type="button" class="title" disabled={missing} onclick={() => play(i)}>
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
	}
</style>
