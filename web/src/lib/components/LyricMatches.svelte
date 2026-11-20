<script lang="ts">
	// A search's lyric matches: each a song found by a line of its lyrics, the line shown between
	// the lines around it, so the user sees the words they remembered (`requirements/search.md`
	// §4). Playing one plays its song from the start, as a song in a list plays, and the songs
	// after it here (`design/search.md` §2.4).
	import type { LyricMatch, PlayContext, TrackSummary } from '$lib/api/types';
	import { fullText } from '$lib/full-text';
	import { player } from '$lib/player.svelte';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import Icon from './Icon.svelte';

	let {
		matches,
		context,
		sentinel,
		onplay
	}: {
		matches: LyricMatch[];
		/** What a match plays in: the search's lyrics. */
		context: PlayContext;
		/** Loads more as the list's end comes into view. */
		sentinel?: (el: Element) => () => void;
		/** A match played, which acts on the search. */
		onplay?: (track: TrackSummary) => void;
	} = $props();

	const tracks = $derived(matches.map((match) => match.track));

	function play(index: number) {
		player.play(tracks, index, context);
		onplay?.(tracks[index]);
	}
</script>

<ol class="matches">
	{#each matches as match, i (match.track.id)}
		{@const track = match.track}
		<li>
			<button type="button" class="play" aria-label="Play {track.title}" onclick={() => play(i)}>
				<Artwork
					image={track.album?.image ?? null}
					seed={track.album?.id ?? track.id}
					size={56}
					icon="song"
				/>
				<span class="overlay"><Icon name="play" size={20} /></span>
			</button>
			<div class="text">
				<div class="song">
					<span class="title" {@attach fullText}>{track.title}</span>
					<span class="artists muted"><ArtistLinks artists={track.artists} /></span>
				</div>
				<div class="lines">
					{#each match.before as line, j (j)}<p class="context muted">{line}</p>{/each}
					<p class="line">{match.line.text}</p>
					{#each match.after as line, j (j)}<p class="context muted">{line}</p>{/each}
				</div>
			</div>
		</li>
	{/each}
</ol>
{#if sentinel}
	<div {@attach sentinel} aria-hidden="true"></div>
{/if}

<style>
	.matches {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin: 0;
		padding: 0;
		list-style: none;
	}

	li {
		display: flex;
		align-items: flex-start;
		gap: var(--space-3);
		padding: var(--space-2);
		border-radius: var(--radius-m);
	}

	li:hover {
		background: var(--surface-hover);
	}

	.play {
		position: relative;
		flex-shrink: 0;
		width: 56px;
		padding: 0;
		border: 0;
		border-radius: var(--radius-s);
		background: none;
		cursor: pointer;
		overflow: hidden;
	}

	.overlay {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		background: rgb(0 0 0 / 0.45);
		color: #fff;
		opacity: 0;
	}

	.play:hover .overlay,
	.play:focus-visible .overlay {
		opacity: 1;
	}

	.text {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		min-width: 0;
	}

	.song {
		display: flex;
		align-items: baseline;
		gap: var(--space-2);
		min-width: 0;
		white-space: nowrap;
	}

	.title {
		overflow: hidden;
		font-weight: 600;
		text-overflow: ellipsis;
	}

	.artists {
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.lines p {
		margin: 0;
		overflow-wrap: anywhere;
	}

	/* The line found, between the lines around it. */
	.line {
		font-weight: 600;
	}

	.context {
		font-size: 14px;
	}
</style>
