<script lang="ts">
	import { listAlbums } from '$lib/api/client';
	import { albumCard } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';

	let { data } = $props();

	const albums = $derived(
		new Paged(data.albums, (cursor) => listAlbums(fetch, data.lib, { sort: 'name', cursor }))
	);
</script>

<svelte:head><title>Albums · Jewelcase</title></svelte:head>

<PageHeader title="Albums" subtitle={plural(albums.total, 'album')} toggle />
<CollectionView
	cards={albums.items.map(albumCard)}
	sentinel={albums.sentinel}
	empty="No albums yet. They appear here as the library is scanned."
/>
