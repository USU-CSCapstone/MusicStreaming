<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { createPlaylist, listPlaylists } from '$lib/api/client';
	import type { PlaylistCreate } from '$lib/api/types';
	import { playlistCard } from '$lib/cards';
	import CollectionView from '$lib/components/CollectionView.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { deleted } from '$lib/deletions.svelte';
	import PlaylistDetailsDialog, {
		type PlaylistDetails
	} from '$lib/components/PlaylistDetailsDialog.svelte';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';

	let { data } = $props();

	const playlists = $derived(
		new Paged(data.playlists, (cursor) => listPlaylists(fetch, data.lib, { cursor }))
	);
	// Deleted ones go at once, while their Undo is still on offer.
	const shown = $derived(playlists.items.filter((p) => !deleted.has(p.id)));
	const total = $derived(playlists.total - (playlists.items.length - shown.length));
	let creating = $state(false);

	/**
	 * The last request and its `Idempotency-Key`. Sending the same request again reuses the key,
	 * so a retry after a lost response still makes one playlist; a changed one gets a new key.
	 */
	let sent: { request: string; key: string } | null = null;

	/** A new playlist, empty to start with (`requirements/playlists.md` §2), opened at once. */
	async function create({ title, description }: PlaylistDetails) {
		const body: PlaylistCreate = { title };
		if (description) body.description = description;
		const request = JSON.stringify(body);
		if (sent?.request !== request) sent = { request, key: crypto.randomUUID() };
		const playlist = await createPlaylist(fetch, data.lib, body, sent.key);
		// The dialog stays up, busy, until the new playlist's page replaces this one.
		await goto(resolve('/playlists/[id]', { id: playlist.id }));
	}
</script>

<svelte:head><title>Playlists · Jewelcase</title></svelte:head>

<PageHeader title="Playlists" subtitle={plural(total, 'playlist')} toggle>
	{#snippet actions()}
		<!-- On a phone the label goes, and the button keeps its name. -->
		<button type="button" class="new" aria-label="New playlist" onclick={() => (creating = true)}>
			<Icon name="plus" size={18} />
			<span class="label">New playlist</span>
		</button>
	{/snippet}
</PageHeader>
<CollectionView
	cards={shown.map(playlistCard)}
	sentinel={playlists.sentinel}
	empty="No playlists yet."
/>

{#if creating}
	<PlaylistDetailsDialog
		heading="New playlist"
		action="Create"
		busyAction="Creating…"
		onsave={create}
		onclose={() => (creating = false)}
	/>
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
