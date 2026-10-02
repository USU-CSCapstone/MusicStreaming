<script lang="ts">
	import { listAlbums, listTracks } from '$lib/api/client';
	import { albumCard } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import DetailHeader from '$lib/components/DetailHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import ViewToggle from '$lib/components/ViewToggle.svelte';
	import { artistName, plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';
	import { player } from '$lib/player.svelte';

	let { data } = $props();

	const artist = $derived(data.artist);
	const context = $derived({ type: 'artist', id: artist.id, scope: 'all' } as const);
	const albums = $derived(
		new Paged(data.albums, (cursor) =>
			listAlbums(fetch, data.lib, {
				artistId: artist.id,
				artistCredit: 'owned',
				sort: 'releaseDate',
				order: 'desc',
				cursor
			})
		)
	);
	const tracks = $derived(
		new Paged(data.tracks, (cursor) =>
			listTracks(fetch, data.lib, { artistId: artist.id, sort: 'album', cursor })
		)
	);
	const counts = $derived(
		[
			artist.albumCount > 0 ? plural(artist.albumCount, 'album') : null,
			plural(artist.trackCount, 'song')
		]
			.filter(Boolean)
			.join(' · ')
	);
</script>

<svelte:head><title>{artistName(artist.name)} · Jewelcase</title></svelte:head>

<DetailHeader
	kind="Artist"
	title={artistName(artist.name)}
	image={artist.image}
	seed={artist.id}
	icon="artist"
	round
	onplay={tracks.items.length ? () => player.play(tracks.items, 0, context) : undefined}
>
	<span>{counts}</span>
</DetailHeader>

{#if artist.biography}
	<p class="bio">{artist.biography}</p>
{/if}

{#if albums.items.length}
	<section>
		<header>
			<h2>Albums</h2>
			<ViewToggle />
		</header>
		<CollectionView cards={albums.items.map(albumCard)} sentinel={albums.sentinel} />
	</section>
{/if}

{#if data.appearances.items.length}
	<section>
		<header><h2>Appears On</h2></header>
		<CollectionView cards={data.appearances.items.map(albumCard)} />
	</section>
{/if}

<section>
	<header><h2>Songs</h2></header>
	<TrackList tracks={tracks.items} sentinel={tracks.sentinel} {context} />
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

	.bio {
		max-width: 70ch;
		margin: 0 0 var(--space-6);
		color: var(--text-muted);
		white-space: pre-line;
	}
</style>
