<script lang="ts">
	import { resolve } from '$app/paths';
	import type { ArtistRef } from '$lib/api/types';
	import { artistName } from '$lib/format';
	import { fullText } from '$lib/full-text';

	// Every credit is a link, first artist first (`requirements/conventions.md` §3, §7).
	let { artists }: { artists: ArtistRef[] } = $props();
</script>

<span class="artists" {@attach fullText}>
	{#each artists as artist, i (artist.id)}
		{#if i > 0}<span class="sep">, </span>{/if}
		<a href={resolve('/artists/[id]', { id: artist.id })}>{artistName(artist.name)}</a>
	{/each}
</span>

<style>
	.artists {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
