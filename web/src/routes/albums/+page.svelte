<script lang="ts">
	import { listAlbums } from '$lib/api/client';
	import { albumCard } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import FilterBar from '$lib/components/FilterBar.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { isFiltered, showFiltered } from '$lib/filters';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';

	let { data } = $props();

	const albums = $derived(
		new Paged(data.albums, (cursor) => listAlbums(fetch, data.lib, { ...data.query, cursor }))
	);
</script>

<svelte:head><title>Albums · Jewelcase</title></svelte:head>

<PageHeader title="Albums" subtitle={plural(albums.total, 'album')} toggle />
<FilterBar libraryId={data.lib} filter={data.filter} onchange={showFiltered} />
<CollectionView
	cards={albums.items.map(albumCard)}
	sentinel={albums.sentinel}
	empty={isFiltered(data.filter)
		? 'No albums carry these tags.'
		: 'No albums yet. They appear here as the library is scanned.'}
/>
