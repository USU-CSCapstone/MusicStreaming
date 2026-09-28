<script lang="ts">
	import { listTracks } from '$lib/api/client';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';

	let { data } = $props();

	const tracks = $derived(
		new Paged(data.tracks, (cursor) => listTracks(fetch, data.lib, { sort: 'name', cursor }))
	);
</script>

<svelte:head><title>Songs · Jewelcase</title></svelte:head>

<!-- Songs are always a list: a grid of one album cover per song says nothing. -->
<PageHeader title="Songs" subtitle={plural(tracks.total, 'song')} />
<TrackList
	tracks={tracks.items}
	sentinel={tracks.sentinel}
	empty="No songs yet. They appear here as the library is scanned."
/>
