<script lang="ts">
	import { marks } from '$lib/marks.svelte';
	import { onMount } from 'svelte';
	import type { NavigationTarget } from '@sveltejs/kit';
	import { beforeNavigate } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { view } from '$lib/adding.svelte';
	import { searchSection } from '$lib/api/client';
	import type { PlayContext, SearchResult } from '$lib/api/types';
	import { albumCard, artistCard, playlistCard, type Card } from '$lib/cards';
	import AddToPlaylistButton from '$lib/components/AddToPlaylistButton.svelte';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import SearchBox, { settler } from '$lib/components/SearchBox.svelte';
	import SearchTabs from '$lib/components/SearchTabs.svelte';
	import TopResult from '$lib/components/TopResult.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { DEPENDS } from '$lib/live.svelte';
	import { Paged } from '$lib/paged.svelte';
	import { tagTypePlural } from '$lib/tags';
	import type { Selected } from '$lib/recent';
	import { searchRecipe, type PlayFrom } from '$lib/queue';
	import { SEARCH_TABS, tabHref } from '$lib/search-tabs';
	import { currentSort, showSorted } from '$lib/sort-url';
	import { FIELDS } from '$lib/sorting';
	import { sortTracks } from '$lib/track-sort';
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

	// A tab's matches, a page at a time as the list scrolls, read again when the library changes
	// what its type would find.
	const paged = $derived.by(() => {
		const { tab, section } = data;
		if (!tab || !section) return null;
		return new Paged(
			section.first,
			(cursor, limit) =>
				searchSection(fetch, data.lib, tab, data.q, { ...section.query, cursor, limit }),
			data.lib,
			DEPENDS[tab],
			data.since
		);
	});
	const searched = $derived(Boolean(data.results || data.section));
	const sections = $derived(
		paged && data.tab
			? paged.total
				? [{ type: data.tab, items: paged.items, total: paged.total }]
				: []
			: (data.results?.sections ?? [])
	);
	const libraryEmpty = $derived(data.results?.libraryEmpty ?? data.library?.trackCount === 0);
	const nearMisses = $derived(data.results?.nearMisses ?? []);
	// What nothing found on a tab is called, as the tab is.
	const looking = $derived(
		SEARCH_TABS.find((tab) => tab.type === data.tab)?.label.toLowerCase() ?? 'music'
	);
	const tagTypes = $derived(data.tagTypes ?? []);
	// Songs come in match order; sorted, they play as shown (`requirements/conventions.md` §4.1).
	// All sorts the few it shows itself; the Songs tab's come sorted by the server.
	const sort = $derived(currentSort(FIELDS));
	const songs = $derived.by(() => {
		const found = sections.find((s) => s.type === 'tracks')?.items ?? [];
		const tracks = found.flatMap((r) => (r.track ? [r.track] : []));
		return paged ? tracks : [...sortTracks(tracks, sort)];
	});

	// The songs the search finds, in its ranking, which the server resolves to the end. Unsorted,
	// the songs shown are the first of them; sorted, they play as shown. The top result, a track,
	// always plays first of them.
	const ranked = $derived<PlayFrom | undefined>(
		data.q
			? {
					recipe: searchRecipe(data.q),
					total: sections.find((s) => s.type === 'tracks')?.total ?? songs.length
				}
			: undefined
	);
	const from = $derived(sort ? undefined : ranked);
	const context = $derived<PlayContext>({ type: 'search', query: data.q ?? '', section: 'tracks' });

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

{#if searched && !libraryEmpty}
	<SearchTabs q={data.q} current={data.tab} />
{/if}

{#if !searched}
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
		{#if libraryEmpty}
			<h2>You have no music yet</h2>
			<p class="muted">Songs appear here once the library has been scanned.</p>
		{:else}
			<h2>No {looking} {looking === 'music' ? 'matches' : 'match'} “{data.q}”</h2>
			{#if nearMisses.length}
				<p class="muted">
					Did you mean
					{#each nearMisses as miss, i (miss)}
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
	{#if !data.tab && data.results?.top}
		<TopResult
			result={data.results.top}
			{context}
			from={ranked}
			onact={(selected) => settler.settle(selected)}
		/>
	{/if}
	{#each sections as section (section.type)}
		{@const tab = data.tab ? undefined : SEARCH_TABS.find((t) => t.type === section.type)}
		<section>
			<header>
				<!-- On All, a section leads to its tab, which holds every match (`requirements/search.md` §4). -->
				{#if tab}
					<!-- eslint-disable svelte/no-navigation-without-resolve -- resolved in search-tabs.ts -->
					<h2><a href={tabHref(data.q, tab.type)}>{titles[section.type]}</a></h2>
				{:else}
					<h2>{titles[section.type]}</h2>
				{/if}
				<div class="actions">
					{#if tab && section.total > section.items.length}
						<a class="all" href={tabHref(data.q, tab.type)}>Show all {section.total}</a>
						<!-- eslint-enable svelte/no-navigation-without-resolve -->
					{/if}
					{#if section.type === 'tracks' && everySong}<AddToPlaylistButton what={everySong} />{/if}
					{#if section.type !== 'tracks' && section.type !== 'lyrics'}<ViewToggle />{/if}
				</div>
			</header>
			{#if section.type === 'tracks'}
				<TrackList
					tracks={marks.unhidden(data.lib, songs)}
					sentinel={paged?.sentinel}
					{sort}
					onsort={showSorted}
					{context}
					{from}
					onplay={(track) => settler.settle({ type: 'track', id: track.id })}
				/>
			{:else}
				<CollectionView cards={section.items.flatMap(card)} sentinel={paged?.sentinel} />
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
		gap: var(--space-3);
		margin-bottom: var(--space-4);
	}

	h2 a {
		color: inherit;
		text-decoration: none;
	}

	h2 a:hover,
	.all:hover {
		text-decoration: underline;
	}

	.actions {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.all {
		color: var(--text-muted);
		font-weight: 600;
		text-decoration: none;
		white-space: nowrap;
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
