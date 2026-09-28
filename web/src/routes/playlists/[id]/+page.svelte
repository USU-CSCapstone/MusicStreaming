<script lang="ts">
	import DetailHeader from '$lib/components/DetailHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { formatLength, plural } from '$lib/format';
	import { player } from '$lib/player.svelte';

	let { data } = $props();

	const playlist = $derived(data.playlist);
</script>

<svelte:head><title>{playlist.title} · Jewelcase</title></svelte:head>

<DetailHeader
	kind="Playlist"
	title={playlist.title}
	image={playlist.image}
	seed={playlist.id}
	icon="playlist"
	onplay={data.tracks.length ? () => player.play(data.tracks, 0) : undefined}
>
	{#if playlist.description}<span>{playlist.description}</span>{/if}
	<span>{plural(playlist.trackCount, 'song')} · {formatLength(playlist.durationUs)}</span>
</DetailHeader>

<TrackList tracks={data.tracks} empty="This playlist is empty." />
