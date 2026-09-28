<script lang="ts">
	import { resolve } from '$app/paths';
	import type { SearchResult } from '$lib/api/types';
	import { albumCard, artistCard, playlistCard, type Card } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import SearchBox from '$lib/components/SearchBox.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import ViewToggle from '$lib/components/ViewToggle.svelte';

	let { data } = $props();

	const titles = {
		tracks: 'Songs',
		albums: 'Albums',
		artists: 'Artists',
		playlists: 'Playlists',
		lyrics: 'Lyrics'
	};

	function card(r: SearchResult): Card[] {
		if (r.album) return [albumCard(r.album)];
		if (r.artist) return [artistCard(r.artist)];
		if (r.playlist) return [playlistCard(r.playlist)];
		return [];
	}

	const sections = $derived(data.results?.sections ?? []);
</script>

<svelte:head><title>{data.q ? `${data.q} · ` : ''}Search · Jewelcase</title></svelte:head>

<!-- The sidebar holds the box on wide screens; at phone width this page does. -->
<div class="box"><SearchBox /></div>

{#if !data.results}
	<p class="state muted">Search songs, albums, artists, and playlists.</p>
{:else if sections.length === 0}
	<!-- "No music yet" and "nothing matches" call for different responses (`requirements/search.md` §8). -->
	<div class="state">
		{#if data.results.libraryEmpty}
			<h2>You have no music yet</h2>
			<p class="muted">Songs appear here once the library has been scanned.</p>
		{:else}
			<h2>No music matches “{data.q}”</h2>
			{#if data.results.nearMisses.length}
				<p class="muted">
					Did you mean
					{#each data.results.nearMisses as miss, i (miss)}
						{#if i > 0},
						{/if}<a href="{resolve('/search')}?{new URLSearchParams({ q: miss })}">{miss}</a>
					{/each}?
				</p>
			{:else}
				<p class="muted">Check the spelling, or try fewer words.</p>
			{/if}
		{/if}
	</div>
{:else}
	{#each sections as section (section.type)}
		<section>
			<header>
				<h2>{titles[section.type]}</h2>
				{#if section.type !== 'tracks' && section.type !== 'lyrics'}<ViewToggle />{/if}
			</header>
			{#if section.type === 'tracks'}
				<TrackList tracks={section.items.flatMap((r) => (r.track ? [r.track] : []))} />
			{:else}
				<CollectionView cards={section.items.flatMap(card)} />
			{/if}
		</section>
	{/each}
{/if}

<style>
	.box {
		display: none;
		margin-bottom: var(--space-5);
	}

	section {
		margin-bottom: var(--space-6);
	}

	section header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: var(--space-4);
	}

	h2 {
		font-size: 20px;
	}

	.state {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin-top: 10vh;
		text-align: center;
	}

	.state p {
		margin: 0;
	}

	.state a {
		color: var(--accent);
	}

	@media (max-width: 767px) {
		.box {
			display: block;
		}
	}
</style>
