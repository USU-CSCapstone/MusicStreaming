<script lang="ts">
	import { listPlaylists } from '$lib/api/client';
	import { playlistCard } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';

	let { data } = $props();

	const playlists = $derived(
		new Paged(data.playlists, (cursor) => listPlaylists(fetch, data.lib, { cursor }))
	);
</script>

<svelte:head><title>Playlists · Jewelcase</title></svelte:head>

<PageHeader title="Playlists" subtitle={plural(playlists.total, 'playlist')} toggle />
<CollectionView
	cards={playlists.items.map(playlistCard)}
	sentinel={playlists.sentinel}
	empty="No playlists yet."
/>
