<script lang="ts">
	import { listArtists } from '$lib/api/client';
	import { artistCard } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import FilterBar from '$lib/components/FilterBar.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { isFiltered, showFiltered } from '$lib/filters';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';

	let { data } = $props();

	const artists = $derived(
		new Paged(data.artists, (cursor) => listArtists(fetch, data.lib, { ...data.query, cursor }))
	);
</script>

<svelte:head><title>Artists · Jewelcase</title></svelte:head>

<PageHeader title="Artists" subtitle={plural(artists.total, 'artist')} toggle />
<FilterBar libraryId={data.lib} filter={data.filter} onchange={showFiltered} />
<CollectionView
	cards={artists.items.map(artistCard)}
	sentinel={artists.sentinel}
	empty={isFiltered(data.filter)
		? 'No artists carry these tags.'
		: 'No artists yet. They appear here as the library is scanned.'}
/>
