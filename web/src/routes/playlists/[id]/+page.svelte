<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import DetailHeader from '$lib/components/DetailHeader.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import PlaylistDetailsDialog, {
		type PlaylistDetails
	} from '$lib/components/PlaylistDetailsDialog.svelte';
	import TrackList, { type RowMove } from '$lib/components/TrackList.svelte';
	import { deleteWithUndo } from '$lib/deletions.svelte';
	import { formatLength, plural } from '$lib/format';
	import { toast } from '$lib/toast.svelte';
	import { player } from '$lib/player.svelte';
	import { EditablePlaylist, rowMoveOps } from '$lib/playlist.svelte';

	let { data } = $props();

	// Edits show at once and are saved behind them (`requirements/playlists.md` §4).
	const list = $derived(new EditablePlaylist(fetch, data.loaded));
	const playlist = $derived(list.playlist);
	// External tracks from plugins are not shown yet (`requirements/plugins.md` §10), so a row's
	// index is not always its position in the playlist: `positions` maps one to the other.
	const rows = $derived(
		list.items.flatMap((item, position) => (item.track ? [{ track: item.track, position }] : []))
	);
	const tracks = $derived(rows.map((r) => r.track));
	const context = $derived({ type: 'playlist', id: playlist.id } as const);
	let editing = $state(false);

	/** Sends only what changed; nothing changed is no edit. */
	function save({ title, description }: PlaylistDetails) {
		const change: { title?: string; description?: string | null } = {};
		if (title !== playlist.title) change.title = title;
		if (description !== playlist.description) change.description = description;
		if (Object.keys(change).length) list.edit(change);
		editing = false;
	}

	/** Removals can be undone (`requirements/playlists.md` §4). */
	function removeRows(indexes: number[]) {
		const undo = list.edit({
			ops: [{ op: 'remove', positions: indexes.map((i) => rows[i].position) }]
		});
		toast(`Removed ${plural(indexes.length, 'song')}`, { action: { label: 'Undo', run: undo } });
	}

	function moveRows(moves: RowMove[]) {
		list.edit({ ops: rowMoveOps(list.items, moves) });
	}

	/** No confirmation: Undo is offered instead (`requirements/playlists.md` §4). */
	function remove() {
		deleteWithUndo(fetch, playlist, () => list.idle());
		// Replacing this page, so Back does not lead to a deleted playlist.
		void goto(resolve('/playlists'), { replaceState: true });
	}
</script>

<svelte:head><title>{playlist.title} · Jewelcase</title></svelte:head>

<DetailHeader
	kind="Playlist"
	title={playlist.title}
	image={playlist.image}
	seed={playlist.id}
	icon="playlist"
	onplay={tracks.length ? () => player.play(tracks, 0, context) : undefined}
>
	{#if playlist.description}<span>{playlist.description}</span>{/if}
	<span>{plural(playlist.trackCount, 'song')} · {formatLength(playlist.durationUs)}</span>
	{#snippet actions()}
		<button type="button" class="action" onclick={() => (editing = true)}>
			<Icon name="edit" size={16} /> Edit
		</button>
		<!-- Pinned playlists list first (`requirements/playlists.md` §7). -->
		<button
			type="button"
			class="action icon"
			aria-label="Pin"
			title={playlist.pinned
				? 'Pinned to the top of your playlists'
				: 'Pin to the top of your playlists'}
			aria-pressed={playlist.pinned}
			onclick={() => list.edit({ pinned: !playlist.pinned })}
		>
			<Icon name="pin" size={16} />
		</button>
		<button
			type="button"
			class="action icon"
			aria-label="Delete playlist"
			title="Delete playlist"
			onclick={remove}
		>
			<Icon name="trash" size={16} />
		</button>
	{/snippet}
</DetailHeader>

<TrackList
	{tracks}
	empty="This playlist is empty."
	{context}
	editable
	onremove={removeRows}
	onmove={moveRows}
/>

{#if editing}
	<PlaylistDetailsDialog
		heading="Edit details"
		action="Save"
		busyAction="Saving…"
		initial={{ title: playlist.title, description: playlist.description }}
		onsave={save}
		onclose={() => (editing = false)}
	/>
{/if}

<style>
	.action {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		height: 38px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: 19px;
		background: var(--surface);
		color: var(--text);
		font-weight: 500;
		cursor: pointer;
	}

	.action:hover {
		background: var(--surface-hover);
	}

	.action.icon {
		justify-content: center;
		width: 38px;
		padding: 0;
	}

	.action[aria-pressed='true'] {
		border-color: var(--accent);
		background: var(--accent-soft);
		color: var(--accent);
	}
</style>
