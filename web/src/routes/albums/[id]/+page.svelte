<script lang="ts">
	import ArtistLinks from '$lib/components/ArtistLinks.svelte';
	import DetailHeader from '$lib/components/DetailHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { albumTitle, formatLength, plural, releaseYear } from '$lib/format';
	import { player } from '$lib/player.svelte';

	let { data } = $props();

	const album = $derived(data.album);
	const context = $derived({ type: 'album', id: album.id } as const);
	const kinds = { album: 'Album', ep: 'EP', single: 'Single', compilation: 'Compilation' };
	const kind = $derived(
		[kinds[album.type], releaseYear(album.releaseDate)].filter(Boolean).join(' · ')
	);

	// Incomplete is information, never an error (`requirements/albums.md` §4).
	const completeness = $derived.by(() => {
		const parts = [];
		if (album.trackTotal !== null && album.trackCount < album.trackTotal) {
			parts.push(`${album.trackCount} of ${album.trackTotal} tracks`);
		}
		if (album.discTotal !== null && album.discCount < album.discTotal) {
			parts.push(`${album.discCount} of ${album.discTotal} discs`);
		}
		return parts.join(' · ');
	});
</script>

<svelte:head><title>{albumTitle(album.title)} · Jewelcase</title></svelte:head>

<DetailHeader
	{kind}
	title={albumTitle(album.title)}
	image={album.image}
	seed={album.id}
	icon="album"
	onplay={data.tracks.length ? () => player.play(data.tracks, 0, context) : undefined}
>
	<span class="artists"><ArtistLinks artists={album.artists} /></span>
	<span>
		{plural(album.trackCount, 'song')} · {formatLength(album.durationUs)}
		{#if completeness}· <span class="incomplete">{completeness}</span>{/if}
	</span>
</DetailHeader>

<TrackList tracks={data.tracks} variant="album" {context} />

<style>
	.artists {
		color: var(--text);
		font-weight: 600;
	}

	.incomplete {
		color: var(--text);
	}
</style>
