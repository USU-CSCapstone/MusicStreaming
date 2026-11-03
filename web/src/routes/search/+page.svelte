<script lang="ts">
	import { onMount } from 'svelte';
	import type { NavigationTarget } from '@sveltejs/kit';
	import { beforeNavigate } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { view } from '$lib/adding.svelte';
	import type { SearchResult } from '$lib/api/types';
	import { albumCard, artistCard, playlistCard, type Card } from '$lib/cards';
	import AddToPlaylistButton from '$lib/components/AddToPlaylistButton.svelte';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import FilterPicker from '$lib/components/FilterPicker.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import SearchBox, { settler } from '$lib/components/SearchBox.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { UNFILTERED, isFiltered, showFiltered, shownFilter } from '$lib/filters';
	import { tagTypePlural } from '$lib/tags';
	import type { Selected } from '$lib/recent';
	import { searchRecipe, type PlayFrom } from '$lib/queue';
	import { currentSort, showSorted } from '$lib/sort-url';
	import { FIELDS, sortTracks } from '$lib/sorting';
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
	const filtered = $derived(isFiltered(data.filter));
	// The filter narrows every section, so its controls sit above them all, and stay when it
	// leaves nothing (`requirements/tags.md` §7).
	const filter = $derived(shownFilter(data.filter));
	const tagTypes = $derived(data.tagTypes ?? []);
	// Songs come in match order; sorted, they play as shown (`requirements/conventions.md` §4.1).
	const sort = $derived(currentSort(FIELDS));
	const songs = $derived.by(() => {
		const found = sections.find((s) => s.type === 'tracks')?.items ?? [];
		return [
			...sortTracks(
				found.flatMap((r) => (r.track ? [r.track] : [])),
				sort
			)
		];
	});

	// Unsorted, the songs are the first of what the search finds under the loaded filter, in its
	// ranking, which the server resolves to the end. Sorted, they play as shown.
	const from = $derived<PlayFrom | undefined>(
		sort || !data.q
			? undefined
			: {
					recipe: searchRecipe(data.q, data.filter),
					total: sections.find((s) => s.type === 'tracks')?.total ?? songs.length
				}
	);

	// All the songs the search finds, in its ranking, which only queues. Sorted, there is no
	// recipe for the order shown, and the rows queue one by one.
	const everySong = $derived(from ? view('all songs found', from.total, from.recipe) : null);

	// A search is kept once it settles (`$lib/recent`): the user opens or plays one of its
	// results, submits it in the box, or leaves it after it has stood a moment.
	$effect(() => settler.shown(data.library?.id ?? '', data.q ?? ''));

	/** The album or artist among these results that `to` opens, if it is one. */
	function opened(to: NavigationTarget | null): Selected | undefined {
		const id = to?.params?.id;
		const route = to?.route.id;
		const type = route === '/albums/[id]' ? 'album' : route === '/artists/[id]' ? 'artist' : null;
		if (!id || !type) return undefined;
		const found = sections.some((s) => s.items.some((r) => r[type]?.id === id));
		return found ? { type, id } : undefined;
	}

	beforeNavigate(({ to, willUnload }) => {
		// Another query is still the same search.
		if (to?.route.id === '/search') return;
		const selected = opened(to);
		if (selected) settler.settle(selected);
		settler.leave(willUnload);
	});

	// Closing the page or the app, which a phone may do without warning.
	onMount(() => {
		const closing = () => settler.leave(true);
		addEventListener('pagehide', closing);
		return () => removeEventListener('pagehide', closing);
	});
</script>

<svelte:head><title>{data.q ? `${data.q} · ` : ''}Search · Jewelcase</title></svelte:head>

<!-- The sidebar holds the box on wide screens; at phone width this page does. -->
<div class="box"><SearchBox /></div>

{#if data.results && tagTypes.length}
	<div class="filters" role="group" aria-label="Filter by tags">
		{#each tagTypes as type (type.key)}
			<FilterPicker
				libraryId={data.library?.id ?? ''}
				{type}
				variant="button"
				{filter}
				onfilter={showFiltered}
			/>
		{/each}
		{#if isFiltered(filter)}
			<button type="button" class="link" onclick={() => showFiltered({ ...UNFILTERED })}>
				Clear filters
			</button>
		{/if}
	</div>
{/if}

{#if !data.results}
	<p class="state muted">Search songs, albums, artists, and playlists.</p>
	<!-- Each type's tags to browse, which on a phone, without the sidebar, are only here. -->
	{#if tagTypes.length}
		<nav class="browse" aria-label="Browse tags">
			{#each tagTypes as type (type.key)}
				<a href="{resolve('/tags')}?{new URLSearchParams({ type: type.key })}">
					<Icon name="tag" size={18} />
					{tagTypePlural(type.key, tagTypes)}
				</a>
			{/each}
		</nav>
	{/if}
{:else if sections.length === 0}
	<!-- "No music yet" and "nothing matches" call for different responses (`requirements/search.md` §8). -->
	<div class="state">
		{#if data.results.libraryEmpty}
			<h2>You have no music yet</h2>
			<p class="muted">Songs appear here once the library has been scanned.</p>
		{:else}
			<h2>No music matches “{data.q}”{filtered ? ' with these tags' : ''}</h2>
			{#if filtered}
				<p class="muted">
					Try taking a tag out, or
					<button type="button" class="link" onclick={() => showFiltered({ ...UNFILTERED })}>
						search without them</button
					>.
				</p>
			{:else if data.results.nearMisses.length}
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
				{#if section.type === 'tracks' && everySong}<AddToPlaylistButton what={everySong} />{/if}
				{#if section.type !== 'tracks' && section.type !== 'lyrics'}<ViewToggle />{/if}
			</header>
			{#if section.type === 'tracks'}
				<TrackList
					tracks={songs}
					{sort}
					onsort={showSorted}
					filter={data.filter}
					context={{
						type: 'search',
						query: data.q ?? '',
						section: 'tracks',
						...(filtered && {
							filter: { tagAll: data.filter.all, tagAny: data.filter.any }
						})
					}}
					{from}
					onplay={(track) => settler.settle({ type: 'track', id: track.id })}
				/>
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

	.filters {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
		margin-bottom: var(--space-5);
	}

	.browse {
		display: flex;
		flex-wrap: wrap;
		justify-content: center;
		gap: var(--space-2);
		margin-top: var(--space-5);
	}

	.browse a {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		min-height: 44px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: 22px;
		background: var(--surface);
		color: var(--text);
		text-decoration: none;
	}

	.browse a:hover {
		background: var(--surface-hover);
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

	.link {
		padding: 0;
		border: 0;
		background: none;
		color: var(--accent);
		font: inherit;
		cursor: pointer;
	}
</style>
