<script lang="ts">
	import { listPlaylists } from '$lib/api/client';
	import { playlistCard } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import NewPlaylistDialog from '$lib/components/NewPlaylistDialog.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';

	let { data } = $props();

	const playlists = $derived(
		new Paged(data.playlists, (cursor) => listPlaylists(fetch, data.lib, { cursor }))
	);
	let creating = $state(false);
</script>

<svelte:head><title>Playlists · Jewelcase</title></svelte:head>

<PageHeader title="Playlists" subtitle={plural(playlists.total, 'playlist')} toggle>
	{#snippet actions()}
		<!-- On a phone the label goes, and the button keeps its name. -->
		<button type="button" class="new" aria-label="New playlist" onclick={() => (creating = true)}>
			<Icon name="plus" size={18} />
			<span class="label">New playlist</span>
		</button>
	{/snippet}
</PageHeader>
<CollectionView
	cards={playlists.items.map(playlistCard)}
	sentinel={playlists.sentinel}
	empty="No playlists yet."
/>

{#if creating}
	<NewPlaylistDialog libraryId={data.lib} onclose={() => (creating = false)} />
{/if}

<style>
	.new {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		height: 34px;
		padding: 0 var(--space-3) 0 var(--space-2);
		border: 0;
		border-radius: var(--radius-m);
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 500;
		cursor: pointer;
	}

	@media (max-width: 767px) {
		.new {
			width: 34px;
			padding: 0;
			justify-content: center;
		}

		.label {
			display: none;
		}
	}
</style>
