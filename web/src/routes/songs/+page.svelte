<script lang="ts">
	import { marks } from '$lib/marks.svelte';
	import { view } from '$lib/adding.svelte';
	import { listTracks } from '$lib/api/client';
	import AddToPlaylistButton from '$lib/components/AddToPlaylistButton.svelte';
	import type { PlayContext } from '$lib/api/types';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { isFiltered, showFiltered } from '$lib/filters';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';
	import { libraryRecipe, type PlayFrom } from '$lib/queue';
	import { showSorted } from '$lib/sort-url';

	let { data } = $props();

	const tracks = $derived(
		new Paged(data.tracks, (cursor) => listTracks(fetch, data.lib, { ...data.query, cursor }))
	);
	const filtered = $derived(isFiltered(data.filter));

	// A filtered list is a filtered view, which a play records as what it was played from
	// (`requirements/playlists.md` Overview, `requirements/analytics.md` §1).
	const context: PlayContext = $derived(
		filtered
			? { type: 'filter', filter: { tagAll: data.filter.all, tagAny: data.filter.any } }
			: { type: 'library' }
	);
	// The library under the loaded filter, in the order shown, as the server resolves it.
	const from = $derived<PlayFrom>({
		recipe: libraryRecipe({ sort: data.query.sort, order: data.query.order }, data.filter),
		total: tracks.total
	});
	// All of them, in the order shown, which only queues: there is no entity to add to a playlist.
	const all = $derived(view('all songs', tracks.total, from.recipe));
</script>

<svelte:head><title>Songs · Jewelcase</title></svelte:head>

<!-- Songs are always a list: a grid of one album cover per song says nothing. -->
<PageHeader title="Songs" subtitle={plural(tracks.total, 'song')}>
	{#snippet actions()}
		{#if tracks.total}<AddToPlaylistButton what={all} />{/if}
	{/snippet}
</PageHeader>
<TrackList
	tracks={marks.unhidden(data.lib, tracks.items)}
	sentinel={tracks.sentinel}
	sort={data.sort}
	onsort={showSorted}
	filter={data.filter}
	onfilter={showFiltered}
	{context}
	{from}
	empty={filtered
		? 'No songs carry these tags.'
		: 'No songs yet. They appear here as the library is scanned.'}
/>
