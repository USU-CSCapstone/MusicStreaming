<script lang="ts">
	import HiddenSongs from '$lib/components/HiddenSongs.svelte';

	let { data } = $props();
</script>

<svelte:head><title>Hidden Songs · Jewelcase</title></svelte:head>

<HiddenSongs library={data.lib} first={data.hidden} since={data.since} />
