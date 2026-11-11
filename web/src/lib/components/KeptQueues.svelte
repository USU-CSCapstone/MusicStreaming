<script lang="ts">
	import { onMount } from 'svelte';
	import { ApiError, getAlbum, getArtist, getPlaylist, listKeptQueues } from '$lib/api/client';
	import type { KeptQueue, TrackSummary } from '$lib/api/types';
	import { plural } from '$lib/format';
	import { player } from '$lib/player.svelte';
	import { ago, contextLabel, namedBy } from '$lib/queue/kept';
	import { lookup, trackKey } from '$lib/queue/summaries';
	import { realtime } from '$lib/realtime.svelte';
	import { sender } from '$lib/sync/sender';
	import Icon from './Icon.svelte';

	// The account's kept queues, the ones a start or a restore replaced after they were adjusted
	// (`requirements/queue.md` §6): each shows what it was playing, how much it holds and when it
	// was last active. Restoring one makes it the active queue and plays it here where it was left;
	// deleting is one press, a change the log sends in its turn with the queue's others. The list is
	// the server's, so it needs the connection: it is fetched again when the server says it changed
	// (`queuesChanged`) and when the connection returns.

	let { onrestored }: { onrestored?: () => void } = $props();

	let items = $state<KeptQueue[]>([]);
	/** The cursor of the next page; null at the end. */
	let more = $state<string | null>(null);
	let status = $state<'loading' | 'ready' | 'failed'>('loading');
	/** Why the list could not be had, in words. */
	let failure = $state('');
	/** What each entry was playing, by `trackKey`; null for a song that cannot be read. */
	let tracks = $state.raw(new Map<string, TrackSummary | null>());
	/** The names of albums, artists and playlists, by `<kind>/<id>`. */
	let names = $state<Record<string, string>>({});
	/** The queue being restored, so a second press waits. */
	let restoring = $state<string | null>(null);
	/** Why the last restore or delete did not happen. */
	let problem = $state('');
	let loadingMore = $state(false);
	/** Counts loads, so an answer to an earlier one is ignored. */
	let loads = 0;
	/** The queues deleted here that the server may not have heard of yet, left out of every load. */
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- not shown, only read by a load
	const deleted = new Set<string>();

	/** Why a request failed, in words: the connection, or the server's own message. */
	const whyNot = (error: unknown) =>
		error instanceof ApiError ? error.message : 'Kept queues need a connection to the server.';

	/**
	 * Loads the list from its first page, as far as it was already shown, so a change does not send
	 * the person back to the top. The answer is ignored if a later load began.
	 */
	async function load() {
		const load = ++loads;
		const wanted = Math.max(items.length, 1);
		try {
			const got: KeptQueue[] = [];
			let cursor: string | undefined;
			let next: string | null = null;
			do {
				const page = await listKeptQueues(fetch, cursor);
				got.push(...page.items);
				next = page.nextCursor;
				cursor = next ?? undefined;
			} while (next && got.length < wanted);
			if (load !== loads) return;
			items = got.filter((entry) => !deleted.has(entry.queueId));
			more = next;
			status = 'ready';
			void describe(got);
		} catch (error) {
			if (load !== loads) return;
			failure = whyNot(error);
			status = 'failed';
		}
	}

	/** Loads the page after the last one. */
	async function loadMore() {
		if (!more || loadingMore) return;
		loadingMore = true;
		const load = loads;
		try {
			const page = await listKeptQueues(fetch, more);
			if (load !== loads) return;
			items = [...items, ...page.items.filter((entry) => !deleted.has(entry.queueId))];
			more = page.nextCursor;
			void describe(page.items);
		} catch (error) {
			problem = whyNot(error);
		} finally {
			loadingMore = false;
		}
	}

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
		deleted.add(entry.queueId);
		items = items.filter((other) => other.queueId !== entry.queueId);
		sender.enqueue('deleteKeptQueue', { queueId: entry.queueId });
	}

	onMount(() => {
		void load();
		const unhear = realtime.on((message) => {
			if (message.type === 'queuesChanged' || message.type === 'hello') void load();
		});
		const unsettle = sender.onSettled(({ name, args }, outcome) => {
			if (name !== 'deleteKeptQueue') return;
			deleted.delete((args as { queueId: string }).queueId);
			if (outcome !== 'sent') void load();
		});
		return () => {
			unhear();
			unsettle();
		};
	});
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
	{#if status === 'loading'}
		<p class="muted note">Loading…</p>
	{:else if status === 'failed' && items.length === 0}
		<div class="note" role="status">
			<p class="muted">{failure}</p>
			<button type="button" class="retry" onclick={() => load()}>Try again</button>
		</div>
	{:else if items.length === 0}
		<p class="muted note">
			Queues you leave behind are kept here for 30 days, to go back to where you were.
		</p>
	{:else}
		<ul aria-label="Kept queues">
			{#each items as entry (entry.queueId)}
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
		{#if more}
			<button type="button" class="retry" disabled={loadingMore} onclick={loadMore}>
				Show more
			</button>
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

	.note p {
		margin: 0;
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
