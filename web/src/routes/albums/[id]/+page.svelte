<script lang="ts">
	import { collection } from '$lib/adding.svelte';
	import AddToPlaylistButton from '$lib/components/AddToPlaylistButton.svelte';
	import ArtistLinks from '$lib/components/ArtistLinks.svelte';
	import DetailHeader from '$lib/components/DetailHeader.svelte';
	import TagLinks from '$lib/components/TagLinks.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { currentFilter, filterByTags, isFiltered, showFiltered, shownFilter } from '$lib/filters';
	import { albumTitle, formatLength, plural, releaseYear } from '$lib/format';
	import { knownTags } from '$lib/known-tags.svelte';
	import { player } from '$lib/player.svelte';
	import { albumRecipe, type PlayFrom } from '$lib/queue';
	import { currentSort, showSorted } from '$lib/sort-url';
	import { ALBUM_FIELDS, serverSort, sortTracks } from '$lib/sorting';

	let { data } = $props();

	const album = $derived(data.album);
	const context = $derived({ type: 'album', id: album.id } as const);
	// Unsorted is disc then track order (`requirements/albums.md` §4); sorted, it plays as shown
	// (`requirements/conventions.md` §4.1).
	const sort = $derived(currentSort(ALBUM_FIELDS));
	// The album holds its tracks, so it filters them itself, as the server would, and at once
	// for a filter just chosen (`requirements/tags.md` §7).
	const filter = $derived(shownFilter(currentFilter()));
	const shown = $derived(
		filterByTags(
			sortTracks(data.tracks, sort),
			filter,
			(track) => track.tags,
			(tag) => knownTags.get(data.lib, tag)?.type
		)
	);
	// The server resolves the album in the order shown: within the album, or as sorted. Filtered,
	// it plays as shown, since a recipe cannot name tags yet.
	const from = $derived<PlayFrom | undefined>(
		isFiltered(filter)
			? undefined
			: {
					recipe: albumRecipe(album.id, serverSort(sort) ?? undefined),
					total: shown.length
				}
	);
	// The whole album, as the server expands it: every track, in disc then track order.
	const adding = $derived(
		collection(
			'album',
			album.id,
			albumTitle(album.title),
			data.tracks.length,
			async () => data.tracks
		)
	);
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
	onplay={shown.length ? () => player.play(shown, 0, context, from) : undefined}
>
	<span class="artists"><ArtistLinks artists={album.artists} /></span>
	{#if Object.keys(album.tags).length}<TagLinks tags={album.tags} />{/if}
	<span>
		{plural(album.trackCount, 'song')} · {formatLength(album.durationUs)}
		{#if completeness}· <span class="incomplete">{completeness}</span>{/if}
	</span>
	{#snippet actions()}
		{#if data.tracks.length}<AddToPlaylistButton what={adding} />{/if}
	{/snippet}
</DetailHeader>

<TrackList
	tracks={shown}
	variant="album"
	{sort}
	onsort={showSorted}
	{filter}
	onfilter={showFiltered}
	{context}
	{from}
	empty={isFiltered(filter) ? 'No songs on this album carry these tags.' : undefined}
/>

<style>
	.artists {
		color: var(--text);
		font-weight: 600;
	}

	.incomplete {
		color: var(--text);
	}
</style>
