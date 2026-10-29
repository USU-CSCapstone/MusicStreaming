<script lang="ts">
	import { listAlbums, listArtists, listTracks } from '$lib/api/client';
	import { albumCard, artistCard } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import DetailHeader from '$lib/components/DetailHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import ViewToggle from '$lib/components/ViewToggle.svelte';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';
	import { player } from '$lib/player.svelte';
	import { currentSort, showSorted } from '$lib/sort-url';
	import { FIELDS } from '$lib/sorting';
	import { tagTypeName } from '$lib/tags';

	// A tag's page: exactly what carries it, browsable and playable like any other collection
	// (`requirements/tags.md` §5, §7).
	let { data } = $props();

	const tag = $derived(data.tag);
	const context = $derived({ type: 'tag', id: tag.id } as const);
	const sort = $derived(currentSort(FIELDS));
	const albums = $derived(
		new Paged(data.albums, (cursor) =>
			listAlbums(fetch, data.lib, { tag: tag.id, sort: 'name', cursor })
		)
	);
	const artists = $derived(
		new Paged(data.artists, (cursor) =>
			listArtists(fetch, data.lib, { tag: tag.id, sort: 'name', cursor })
		)
	);
	const tracks = $derived(
		new Paged(data.tracks, (cursor) => listTracks(fetch, data.lib, { ...data.songs, cursor }))
	);
	const counts = $derived(
		[
			plural(tag.trackCount, 'song'),
			plural(tag.albumCount, 'album'),
			plural(tag.artistCount, 'artist')
		].join(' · ')
	);
</script>

<svelte:head><title>{tag.name} · Jewelcase</title></svelte:head>

<DetailHeader
	kind={tagTypeName(tag.type, data.tagTypes)}
	title={tag.name}
	image={null}
	seed={tag.id}
	icon="tag"
	onplay={tracks.items.length ? () => player.play(tracks.items, 0, context) : undefined}
>
	<span>{counts}</span>
</DetailHeader>

{#if albums.items.length}
	<section>
		<header>
			<h2>Albums</h2>
			<ViewToggle />
		</header>
		<CollectionView cards={albums.items.map(albumCard)} sentinel={albums.sentinel} />
	</section>
{/if}

{#if artists.items.length}
	<section>
		<header><h2>Artists</h2></header>
		<CollectionView cards={artists.items.map(artistCard)} sentinel={artists.sentinel} />
	</section>
{/if}

<section>
	<header><h2>Songs</h2></header>
	<TrackList
		tracks={tracks.items}
		sentinel={tracks.sentinel}
		{sort}
		onsort={showSorted}
		{context}
		empty="No songs carry this tag any more."
	/>
</section>

<style>
	section {
		margin-bottom: var(--space-6);
	}

	section header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: var(--space-4);
	}

	h2 {
		font-size: 20px;
	}
</style>
