<script lang="ts">
	// Hidden Songs: every song the user hid in a library, newest first, the one place a hidden song
	// is shown, each with Unhide (`requirements/conventions.md` §8). Unhiding takes it out at once,
	// and Undo hides it again.
	import { listHiddenTracks } from '$lib/api/client';
	import type { TrackPage, TrackSummary } from '$lib/api/types';
	import { plural } from '$lib/format';
	import { marks } from '$lib/marks.svelte';
	import { Paged } from '$lib/paged.svelte';
	import { toast } from '$lib/toast.svelte';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';

	let { library, first }: { library: string; first: TrackPage } = $props();

	const hidden = $derived(
		new Paged(first, (cursor) => listHiddenTracks(fetch, library, { cursor }))
	);
	// A song unhidden here, or liked anywhere, since the list loaded is no longer hidden.
	const shown = $derived(hidden.items.filter((track) => marks.hidden(library, track.id, true)));
	const total = $derived(hidden.total - (hidden.items.length - shown.length));

	function unhide(track: TrackSummary) {
		void marks.hide(fetch, library, track, false);
		const undo = () => void marks.hide(fetch, library, track, true);
		toast(`“${track.title}” is back in your library`, { action: { label: 'Undo', run: undo } });
	}
</script>

{#if shown.length}
	<p class="muted">{plural(total, 'hidden song')}</p>
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
				<button type="button" aria-label="Unhide “{track.title}”" onclick={() => unhide(track)}>
					Unhide
				</button>
			</li>
		{/each}
	</ul>
	<div {@attach hidden.sentinel} aria-hidden="true"></div>
{:else}
	<p class="muted">You haven't hidden any songs in this library.</p>
{/if}

<style>
	ul {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: 100%;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	li {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.art {
		width: 40px;
		flex-shrink: 0;
	}

	.text {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
	}

	.title {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	/* As the account page's own buttons. */
	button {
		height: 36px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font-weight: 500;
		cursor: pointer;
	}

	button:hover {
		background: var(--surface-hover);
	}
</style>
