<script lang="ts">
	// Hidden Songs: every song the user hid in a library, newest first, laid out as a playlist is
	// but not played, since a hidden song plays only once it is unhidden. It is the one place a
	// hidden song is shown, each with Unhide (`requirements/conventions.md` §8). Unhiding takes a
	// song out at once, and Undo hides it again.
	import { resolve } from '$app/paths';
	import { listHiddenTracks } from '$lib/api/client';
	import type { TrackPage, TrackSummary } from '$lib/api/types';
	import { albumTitle, plural } from '$lib/format';
	import { marks } from '$lib/marks.svelte';
	import { Paged } from '$lib/paged.svelte';
	import { toast } from '$lib/toast.svelte';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import DetailHeader from './DetailHeader.svelte';

	let { library, first }: { library: string; first: TrackPage } = $props();

	const hidden = $derived(
		new Paged(first, (cursor) => listHiddenTracks(fetch, library, { cursor }))
	);
	// A song unhidden here, or liked anywhere, since the list loaded is no longer hidden.
	const shown = $derived(hidden.items.filter((track) => marks.hidden(library, track.id, true)));
	const total = $derived(hidden.total - (hidden.items.length - shown.length));

	function unhide(song: TrackSummary) {
		// Read as hidden, which a refused unhide goes back to.
		const track = { ...song, hidden: true };
		void marks.hide(fetch, library, track, false);
		const undo = () => void marks.hide(fetch, library, track, true);
		toast(`“${track.title}” is back in your library`, { action: { label: 'Undo', run: undo } });
	}
</script>

<DetailHeader kind="Account" title="Hidden Songs" image={null} seed="hidden" icon="hide">
	<span>{plural(total, 'song')}</span>
</DetailHeader>

{#if shown.length}
	<ul>
		{#each shown as track (track.id)}
			<li>
				<span class="art"
					><Artwork image={track.album.image} seed={track.album.id} size={40} /></span
				>
				<span class="text">
					<span class="title">{track.title}</span>
					<span class="muted"><ArtistLinks artists={track.artists} /></span>
				</span>
				<span class="album muted">
					{#if track.album}
						<a href={resolve('/albums/[id]', { id: track.album.id })}
							>{albumTitle(track.album.title)}</a
						>
					{/if}
				</span>
				<button type="button" aria-label="Unhide “{track.title}”" onclick={() => unhide(track)}>
					Unhide
				</button>
			</li>
		{/each}
	</ul>
	<div {@attach hidden.sentinel} aria-hidden="true"></div>
{:else}
	<p class="muted">
		You haven't hidden any songs in this library. Hide a song from its ⋯ menu, and it's gone from
		your library until you bring it back here.
	</p>
{/if}

<style>
	ul {
		display: flex;
		flex-direction: column;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	li {
		display: grid;
		grid-template-columns: 40px minmax(0, 3fr) minmax(0, 2fr) auto;
		align-items: center;
		gap: var(--space-3);
		min-height: 56px;
		padding: var(--space-1) var(--space-3);
		border-radius: var(--radius-m);
	}

	li:hover {
		background: var(--surface-hover);
	}

	.text {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.title,
	.album {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.album a {
		color: inherit;
	}

	button {
		height: 32px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font-weight: 500;
		cursor: pointer;
	}

	button:hover {
		background: var(--surface-2);
	}

	/* A phone has no room for the album: the title and artists take it. */
	@media (max-width: 767px) {
		li {
			grid-template-columns: 40px minmax(0, 1fr) auto;
			padding-inline: var(--space-2);
		}

		.album {
			display: none;
		}
	}
</style>
