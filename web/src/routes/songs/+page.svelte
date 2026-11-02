<script lang="ts">
	import { songs, view } from '$lib/adding.svelte';
	import { listTracks } from '$lib/api/client';
	import AddToPlaylistButton from '$lib/components/AddToPlaylistButton.svelte';
	import type { PlayContext } from '$lib/api/types';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { isFiltered, showFiltered } from '$lib/filters';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';
	import type { PlayFrom } from '$lib/queue';
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
	// Unfiltered, the library in the order shown, as the server resolves it. Filtered, the songs
	// play as loaded, since a recipe cannot name tags yet.
	const from = $derived<PlayFrom | undefined>(
		filtered
			? undefined
			: {
					recipe: {
						type: 'library',
						filters: {},
						sort: data.query.sort,
						order: data.query.order
					},
					total: tracks.total
				}
	);
	// All of them, in the order shown, which only queues: there is no entity to add to a playlist.
	// Filtered, the songs loaded, since a recipe cannot name tags yet.
	const all = $derived(
		from ? view('all songs', tracks.total, from.recipe) : { ...songs(tracks.items), refs: [] }
	);
</script>

<svelte:head><title>Songs · Jewelcase</title></svelte:head>

<!-- Songs are always a list: a grid of one album cover per song says nothing. -->
<PageHeader title="Songs" subtitle={plural(tracks.total, 'song')}>
	{#snippet actions()}
		{#if tracks.total}<AddToPlaylistButton what={all} />{/if}
	{/snippet}
</PageHeader>
<TrackList
	tracks={tracks.items}
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
