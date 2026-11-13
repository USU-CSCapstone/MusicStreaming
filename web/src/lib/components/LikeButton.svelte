<script lang="ts">
	// A song's heart: filled when the user likes it, and a press likes or unlikes it, at once
	// (`requirements/playlists.md` §11).
	import type { TrackSummary } from '$lib/api/types';
	import { marks } from '$lib/marks.svelte';
	import Icon from './Icon.svelte';

	let {
		library,
		track,
		size = 18
	}: {
		library: string;
		track: Pick<TrackSummary, 'id' | 'title' | 'liked'>;
		size?: number;
	} = $props();

	const liked = $derived(marks.liked(library, track));
</script>

<button
	type="button"
	class="heart"
	class:liked
	aria-label="Like “{track.title}”"
	aria-pressed={liked}
	title={liked ? 'Remove from Liked Songs' : 'Add to Liked Songs'}
	onclick={() => marks.like(library, track, !liked)}
>
	<Icon name={liked ? 'liked' : 'heart'} {size} />
</button>

<style>
	.heart {
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

	.heart:hover {
		background: var(--surface-hover);
		color: var(--text);
	}

	.liked,
	.liked:hover {
		color: var(--accent);
	}
</style>
