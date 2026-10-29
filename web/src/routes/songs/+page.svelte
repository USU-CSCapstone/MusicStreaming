<script lang="ts">
	import { listTracks } from '$lib/api/client';
	import type { PlayContext } from '$lib/api/types';
	import FilterBar from '$lib/components/FilterBar.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import TrackList from '$lib/components/TrackList.svelte';
	import { isFiltered, showFiltered } from '$lib/filters';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';
	import { showSorted } from '$lib/sort-url';

	let { data } = $props();

	const tracks = $derived(
		new Paged(data.tracks, (cursor) => listTracks(fetch, data.lib, { ...data.query, cursor }))
	);
	const filtered = $derived(isFiltered(data.filter));

	// A filtered list is a filtered view, which a play records as what it was played from
	// (`requirements/playlists.md` Overview, `requirements/analytics.md` §1).
	const context: PlayContext = $derived(
		filtered
			? { type: 'filter', filter: { tags: data.filter.tags, tagMatch: data.filter.match } }
			: { type: 'library' }
	);
</script>

<svelte:head><title>Songs · Jewelcase</title></svelte:head>

<!-- Songs are always a list: a grid of one album cover per song says nothing. -->
<PageHeader title="Songs" subtitle={plural(tracks.total, 'song')} />
<FilterBar libraryId={data.lib} filter={data.filter} onchange={showFiltered} />
<TrackList
	tracks={tracks.items}
	sentinel={tracks.sentinel}
	sort={data.sort}
	onsort={showSorted}
	{context}
	empty={filtered
		? 'No songs carry these tags.'
		: 'No songs yet. They appear here as the library is scanned.'}
/>
