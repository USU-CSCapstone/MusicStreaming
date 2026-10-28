<script lang="ts">
	import { untrack } from 'svelte';
	import { listAlbums, listTracks } from '$lib/api/client';
	import { artistTracks, collection } from '$lib/adding.svelte';
	import { albumCard } from '$lib/cards';
	import AddToPlaylistButton from '$lib/components/AddToPlaylistButton.svelte';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import DetailHeader from '$lib/components/DetailHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import ViewToggle from '$lib/components/ViewToggle.svelte';
	import { artistName, plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';
	import { player } from '$lib/player.svelte';
	import { currentSort, showSorted } from '$lib/sort-url';
	import { FIELDS } from '$lib/sorting';
	import { toast } from '$lib/toast.svelte';
	import { _songOrder as songOrder } from './+page';

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
	const sort = $derived(currentSort(FIELDS));
	// The songs as the page loaded them, until another sort fetches its own first page here,
	// leaving the rest of the page as it is. Another artist starts over from what loads.
	let songs = $derived({ order: data.songs, first: data.tracks });
	$effect(() => {
		const order = songOrder(sort);
		const shown = untrack(() => songs.order);
		if (order.sort === shown.sort && order.order === shown.order) return;
		let current = true;
		listTracks(fetch, data.lib, { artistId: artist.id, ...order }).then(
			(first) => {
				if (current) songs = { order, first };
			},
			(e) => toast(`Couldn’t sort songs. ${e instanceof Error ? e.message : ''}`.trim())
		);
		return () => {
			current = false;
		};
	});
	const query = $derived({ type: 'tracks', artistId: artist.id, ...songs.order } as const);
	const tracks = $derived(
		new Paged(songs.first, (cursor) =>
			listTracks(fetch, data.lib, { artistId: artist.id, ...songs.order, cursor })
		)
	);
	// Every song the artist is credited on, as the server expands an artist, which the page
	// shows a page at a time; `tracks.total` counts the same songs.
	const adding = $derived(
		collection(
			'artist',
			artist.id,
			artistName(artist.name),
			tracks.total,
			artistTracks(fetch, data.lib, artist.id)
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
	onplay={tracks.items.length ? () => player.play(tracks.items, 0, context, query) : undefined}
>
	<span>{counts}</span>
	{#snippet actions()}
		{#if tracks.total}<AddToPlaylistButton what={adding} />{/if}
	{/snippet}
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
	<TrackList
		tracks={tracks.items}
		sentinel={tracks.sentinel}
		{sort}
		onsort={showSorted}
		{context}
		{query}
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

	.bio {
		max-width: 70ch;
		margin: 0 0 var(--space-6);
		color: var(--text-muted);
		white-space: pre-line;
	}
</style>
