<script lang="ts">
	import { untrack } from 'svelte';
	import { getAlbum, getArtist, getPlaylist } from '$lib/api/client';
	import type { KeptQueue, TrackSummary } from '$lib/api/types';
	import { plural } from '$lib/format';
	import { player } from '$lib/player.svelte';
	import { queue } from '$lib/queue/client.svelte';
	import { ago, contextLabel, namedBy } from '$lib/queue/kept';
	import { lookup, trackKey } from '$lib/queue/summaries';
	import Icon from './Icon.svelte';

	// The account's kept queues, the ones a start or a restore replaced after they were adjusted
	// (`requirements/queue.md` §6): each shows what it was playing, how much it holds and when it
	// was last active. The list is the device's (`QueueClient.kept`), online and offline: the queue
	// worker keeps it from the server's and from the queues replaced here. Restoring one makes it the
	// active queue at once and plays it here where it was left; deleting is one press, a change the
	// log sends in its turn with the queue's others.

	let { onrestored }: { onrestored?: () => void } = $props();

	/** How many entries are drawn at first, and added by each "Show more". */
	const STEP = 50;

	/** How many entries are drawn. */
	let shown = $state(STEP);
	/** What each entry was playing, by `trackKey`; null for a song that cannot be read. */
	let tracks = $state.raw(new Map<string, TrackSummary | null>());
	/** The names of albums, artists and playlists, by `<kind>/<id>`. */
	let names = $state<Record<string, string>>({});
	/** The queue being restored, so a second press waits. */
	let restoring = $state<string | null>(null);
	/** Why the last restore did not happen. */
	let problem = $state('');

	const items = $derived(queue.kept ?? []);
	const drawn = $derived(items.slice(0, shown));

	// Only the entries drawn are followed: what `describe` reads and learns is not.
	$effect(() => {
		const entries = drawn;
		untrack(() => void describe(entries));
	});

	/** Looks up the songs and the names of `entries`, for what is not already known. */
	async function describe(entries: KeptQueue[]) {
		const refs = entries.flatMap(({ current }) =>
			current?.trackId && !tracks.has(trackKey(current.libraryId, current.trackId))
				? [{ libraryId: current.libraryId, trackId: current.trackId }]
				: []
		);
		if (refs.length > 0) {
			const found = await lookup(refs);
			tracks = new Map([...tracks, ...found]);
		}
		await Promise.all(
			entries.map(async ({ context, current }) => {
				const named = namedBy(context);
				if (!named || !current || `${named.kind}/${named.id}` in names) return;
				const ask = { album: getAlbum, artist: getArtist, playlist: getPlaylist }[named.kind];
				try {
					const found = await ask(fetch, current.libraryId, named.id);
					const name = 'title' in found ? found.title : found.name;
					if (name) names = { ...names, [`${named.kind}/${named.id}`]: name };
				} catch {
					// Without a name the context is described by its kind.
				}
			})
		);
	}

	/** What an entry was playing, in words. */
	function playing({ current }: KeptQueue): string {
		if (!current) return 'Nothing was playing';
		if (!current.trackId) return 'A song that cannot be read';
		return tracks.get(trackKey(current.libraryId, current.trackId))?.title ?? 'A song';
	}

	/** The words for an entry's context. */
	function from({ context }: KeptQueue): string {
		const named = namedBy(context);
		return contextLabel(context, named ? names[`${named.kind}/${named.id}`] : undefined);
	}

	/** Makes `entry` the active queue and plays it here where it was left. */
	async function restore(entry: KeptQueue) {
		if (restoring) return;
		restoring = entry.queueId;
		problem = '';
		const why = await player.restore(entry.queueId);
		restoring = null;
		if (why) problem = why;
		else onrestored?.();
	}

	/**
	 * Deletes `entry` here at once and gives the delete to the log, online or not; one the server
	 * refuses, as the active queue, is told by the log and shows again.
	 */
	function remove(entry: KeptQueue) {
		problem = '';
		queue.deleteKept(entry.queueId);
	}
</script>

<div class="kept">
	{#if problem}
		<div class="problem" role="alert">
			<span>{problem}</span>
			<button type="button" aria-label="Dismiss" onclick={() => (problem = '')}>
				<Icon name="close" size={14} />
			</button>
		</div>
	{/if}
	{#if queue.kept === null}
		<p class="muted note">Loading…</p>
	{:else if items.length === 0}
		<p class="muted note">
			Queues you leave behind are kept here for 30 days, to go back to where you were.
		</p>
	{:else}
		<ul aria-label="Kept queues">
			{#each drawn as entry (entry.queueId)}
				<li>
					<button
						type="button"
						class="restore"
						disabled={restoring !== null}
						aria-busy={restoring === entry.queueId}
						onclick={() => restore(entry)}
					>
						<span class="title"><span class="sr">Restore </span>{playing(entry)}</span>
						<span class="muted detail">{from(entry)}</span>
						<span class="muted detail tabular">
							{plural(entry.upNext, 'song')} up next · {plural(entry.history, 'play')} played ·
							<time datetime={entry.lastActive}>{ago(entry.lastActive)}</time>
						</span>
					</button>
					<button
						type="button"
						class="delete"
						aria-label="Delete {playing(entry)}, {from(entry)}"
						title="Delete"
						onclick={() => remove(entry)}
					>
						<Icon name="trash" size={16} />
					</button>
				</li>
			{/each}
		</ul>
		{#if items.length > shown}
			<button type="button" class="retry" onclick={() => (shown += STEP)}>Show more</button>
		{/if}
	{/if}
</div>

<style>
	.kept {
		display: flex;
		flex: 1;
		flex-direction: column;
		gap: var(--space-2);
		min-height: 0;
		overflow-y: auto;
	}

	ul {
		display: flex;
		flex-direction: column;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	li {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		min-height: 64px;
	}

	.restore {
		display: flex;
		flex: 1;
		flex-direction: column;
		align-items: flex-start;
		gap: 2px;
		min-width: 0;
		padding: var(--space-2) var(--space-3);
		border: 0;
		border-radius: var(--radius-m);
		background: transparent;
		color: var(--text);
		font: inherit;
		text-align: left;
		cursor: pointer;
	}

	.restore:hover:not(:disabled) {
		background: var(--surface-hover);
	}

	.restore:disabled {
		opacity: 0.6;
		cursor: default;
	}

	.title,
	.detail {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.sr {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}

	.detail {
		font-size: 13px;
	}

	.delete {
		display: grid;
		place-items: center;
		flex-shrink: 0;
		width: 40px;
		height: 40px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
	}

	.delete:hover {
		background: var(--surface-hover);
		color: var(--danger);
	}

	.note {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-2);
		margin: 0;
		padding: var(--space-3);
		font-size: 14px;
	}

	.retry {
		align-self: flex-start;
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: transparent;
		color: var(--text);
		font: inherit;
		font-size: 14px;
		cursor: pointer;
	}

	.problem {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--danger);
		border-radius: var(--radius-m);
		color: var(--danger);
		font-size: 13px;
	}

	.problem span {
		flex: 1;
	}

	.problem button {
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: inherit;
		cursor: pointer;
	}
</style>
