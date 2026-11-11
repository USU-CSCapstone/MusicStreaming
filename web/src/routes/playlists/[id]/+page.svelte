<script lang="ts">
	import { untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { collection, opened, playables } from '$lib/adding.svelte';
	import AddToPlaylistButton from '$lib/components/AddToPlaylistButton.svelte';
	import DetailHeader from '$lib/components/DetailHeader.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import PlaylistDetailsDialog, {
		type PlaylistDetails
	} from '$lib/components/PlaylistDetailsDialog.svelte';
	import TrackList, { type RowMove } from '$lib/components/TrackList.svelte';
	import { playlistArt } from '$lib/cards';
	import { deleteWithUndo } from '$lib/deletions.svelte';
	import {
		currentFilter,
		filterByTags,
		isFiltered,
		showFiltered,
		shownFilter,
		tagTypesKnown
	} from '$lib/filters';
	import { formatLength, plural } from '$lib/format';
	import { DEPENDS, Live } from '$lib/live.svelte';
	import type { PlaylistItem, TrackSummary } from '$lib/api/types';
	import { knownTags } from '$lib/known-tags.svelte';
	import { marks } from '$lib/marks.svelte';
	import { toast } from '$lib/toast.svelte';
	import { player } from '$lib/player.svelte';
	import { EditablePlaylist, loadPlaylist, rowMoveOps } from '$lib/playlist.svelte';
	import { playlistRecipe, type PlayFrom } from '$lib/queue';
	import { currentSort, showSorted } from '$lib/sort-url';
	import { PLAYLIST_FIELDS } from '$lib/sorting';
	import { sortRows } from '$lib/track-sort';

	let { data } = $props();

	// Edits show at once and are saved behind them (`requirements/playlists.md` §4).
	const list = $derived(new EditablePlaylist(fetch, data.loaded));
	// An edit from another device reaches this one through the library's feed, and one of this
	// device's through its copy: read again, in place, when it changes on the device, and shown
	// once the device has every edit made here.
	const reread = $derived(
		new Live(
			data.loaded,
			() => loadPlaylist(fetch, data.lib, data.loaded.playlist.id),
			data.lib,
			DEPENDS.playlist,
			data.since
		)
	);
	$effect(() => {
		const loaded = reread.value;
		untrack(() => list.refresh(loaded));
	});
	// Adding to this playlist from its own page goes through `list`, so it shows at once.
	$effect(() => {
		opened.list = list;
		return () => {
			if (opened.list === list) opened.list = null;
		};
	});
	const playlist = $derived(list.playlist);
	// Liked Songs keeps its name and draws a heart; its songs are the user's likes
	// (`requirements/playlists.md` §11).
	const liked = $derived(playlist.kind === 'liked');
	$effect(() => marks.note([playlist]));
	// External tracks from plugins are not shown yet (`requirements/plugins.md` §10), nor are tracks
	// the user hid, which keep their places (`requirements/playlists.md` §3), so a row's index is
	// not always its position in the playlist: each row carries its own.
	// In Liked Songs, a song unliked since it loaded is no longer there.
	const shows = (item: PlaylistItem) =>
		!!item.track &&
		!marks.hidden(data.lib, item.track.id, item.hidden) &&
		(!liked || marks.liked(data.lib, item.track));
	type Row = { track: TrackSummary; position: number; addedAt: string };
	// Read again as any song is liked or hidden, here or elsewhere; one that changes no row leaves
	// the rows the same list, so a selection or a drag in them carries on.
	let lastRows: Row[] = [];
	const rows = $derived.by(() => {
		const next = list.items.flatMap((item, position) =>
			item.track && shows(item)
				? [{ track: item.track, position, addedAt: item.addedAt ?? '' }]
				: []
		);
		const same =
			next.length === lastRows.length &&
			next.every(
				(row, i) =>
					row.track === lastRows[i].track &&
					row.position === lastRows[i].position &&
					row.addedAt === lastRows[i].addedAt
			);
		if (!same) lastRows = next;
		return lastRows;
	});
	const tracks = $derived(rows.map((r) => r.track));
	// Sorting shows the playlist in another order and leaves its own as arranged; it plays as
	// shown (`requirements/conventions.md` §4.1, `requirements/playlists.md` §10).
	const sort = $derived(currentSort(PLAYLIST_FIELDS));
	// Filtering, like sorting, only changes what is shown, and at once for a filter just chosen
	// (`requirements/tags.md` §7).
	const filter = $derived(shownFilter(currentFilter()));
	const filtered = $derived(isFiltered(filter));
	const shown = $derived(
		filterByTags(
			sortRows(
				rows,
				sort,
				(r) => r.track,
				(r) => r.addedAt
			),
			filter,
			(r) => r.track.tags,
			(tag) => knownTags.get(data.lib, tag)?.type
		)
	);
	const shownTracks = $derived(shown.map((r) => r.track));
	const context = $derived({ type: 'playlist', id: playlist.id } as const);
	// The server resolves the playlist in the order shown, under the filter shown. Its tags are
	// read from the tracks and the known tags, so while a tag's type is still unknown the rows
	// shown may differ from the server's, and the songs shown play instead. So do they while edits
	// are unconfirmed, as the server resolves the playlist as it last saved it, and sorted by date
	// added once songs were added here, which the server dated by its own clock.
	const settled = $derived(
		tagTypesKnown(
			tracks,
			filter,
			(track) => track.tags,
			(tag) => knownTags.get(data.lib, tag) !== undefined
		)
	);
	const from = $derived<PlayFrom | undefined>(
		settled && !list.saving && !(sort?.field === 'added' && list.added)
			? { recipe: playlistRecipe(playlist.id, sort, filter), total: shown.length }
			: undefined
	);
	// This playlist as shown, into another or the queue. Its songs here leave out external tracks,
	// which only matters when skipping songs already there. Queued, the server resolves it as shown.
	const adding = $derived({
		...collection(
			'playlist',
			playlist.id,
			playlist.title,
			shown.length,
			async () => shownTracks,
			from?.recipe
		),
		// Filtered, only the songs shown are checked and added to another playlist, matching the rows.
		...(filtered && { refs: playables(shownTracks) })
	});
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
			ops: [{ op: 'remove', positions: indexes.map((i) => shown[i].position) }]
		});
		toast(`Removed ${plural(indexes.length, 'song')}`, { action: { label: 'Undo', run: undo } });
	}

	function moveRows(moves: RowMove[]) {
		list.edit({ ops: rowMoveOps(list.items, moves, shows) });
	}

	/**
	 * No confirmation: Undo is offered instead (`requirements/playlists.md` §4). But deleting Liked
	 * Songs with songs in it unlikes them all, so that is asked first (§11).
	 */
	function remove() {
		if (liked && playlist.trackCount > 0 && !confirming) {
			confirming = true;
			return;
		}
		confirming = false;
		deleteWithUndo(playlist);
		// Replacing this page, so Back does not lead to a deleted playlist.
		void goto(resolve('/playlists'), { replaceState: true });
	}

	let confirming = $state(false);
	let confirm = $state<HTMLDialogElement>();
	$effect(() => {
		if (confirming) confirm?.showModal();
	});
</script>

<svelte:head><title>{playlist.title} · Jewelcase</title></svelte:head>

<DetailHeader
	kind="Playlist"
	title={playlist.title}
	{...playlistArt(playlist)}
	seed={playlist.id}
	onplay={shownTracks.length ? () => player.play(shownTracks, 0, context, from) : undefined}
>
	{#if playlist.description}<span>{playlist.description}</span>{/if}
	<span>{plural(playlist.trackCount, 'song')} · {formatLength(playlist.durationUs)}</span>
	{#snippet actions()}
		{#if !liked}
			<button type="button" class="action" onclick={() => (editing = true)}>
				<Icon name="edit" size={16} /> Edit
			</button>
		{/if}
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
		{#if tracks.length}<AddToPlaylistButton what={adding} />{/if}
	{/snippet}
</DetailHeader>

<TrackList
	tracks={shownTracks}
	added={shown.map((r) => r.addedAt)}
	{sort}
	onsort={showSorted}
	empty={filtered
		? 'No songs in this playlist carry these tags.'
		: liked
			? 'Songs you like appear here. Press a song’s heart to like it.'
			: 'This playlist is empty.'}
	{context}
	{from}
	editable
	onremove={removeRows}
	onmove={moveRows}
	{filter}
	onfilter={showFiltered}
/>

{#if confirming}
	<dialog
		bind:this={confirm}
		aria-labelledby="unlike-all"
		oncancel={(e) => {
			e.preventDefault();
			confirming = false;
		}}
	>
		<h2 id="unlike-all">Delete Liked Songs?</h2>
		<p>
			{playlist.trackCount === 1
				? 'Its song will be unliked.'
				: `All ${playlist.trackCount.toLocaleString()} songs in it will be unliked.`}
			Liking a song brings it back.
		</p>
		<footer>
			<button type="button" onclick={() => (confirming = false)}>Cancel</button>
			<button type="button" class="primary" onclick={remove}>Delete</button>
		</footer>
	</dialog>
{/if}

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

	dialog {
		width: min(400px, calc(100vw - 2 * var(--space-4)));
		padding: var(--space-5);
		border: 0;
		border-radius: var(--radius-l);
		background: var(--surface);
		color: var(--text);
		box-shadow: var(--shadow);
	}

	dialog::backdrop {
		background: #0006;
	}

	dialog h2 {
		font-size: 18px;
	}

	dialog p {
		margin: var(--space-3) 0 var(--space-4);
	}

	footer {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-2);
	}

	footer button {
		height: 36px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font-weight: 500;
		cursor: pointer;
	}

	footer button.primary {
		border-color: var(--accent);
		background: var(--accent);
		color: var(--accent-text);
	}

	.action[aria-pressed='true'] {
		border-color: var(--accent);
		background: var(--accent-soft);
		color: var(--accent);
	}
</style>
