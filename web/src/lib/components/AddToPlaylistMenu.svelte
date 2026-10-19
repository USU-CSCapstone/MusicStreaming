<script lang="ts">
	// Which playlist songs go to, from a row's ⋯ or the selection bar (`requirements/playlists.md`
	// §4): one of the user's own, or a new one made on the spot. Songs already there are warned
	// about first, as adding them again is usually a mistake (§3).
	import { onMount, tick } from 'svelte';
	import { page } from '$app/state';
	import { createPlaylist, listPlaylists } from '$lib/api/client';
	import type { Playlist, PlaylistCreate, TrackSummary } from '$lib/api/types';
	import { addTracks, alreadyIn, describe, playables } from '$lib/adding.svelte';
	import { deleted } from '$lib/deletions.svelte';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';
	import { toast } from '$lib/toast.svelte';
	import Icon from './Icon.svelte';
	import PlaylistDetailsDialog, { type PlaylistDetails } from './PlaylistDetailsDialog.svelte';

	let {
		tracks,
		anchor,
		onclose
	}: {
		tracks: TrackSummary[];
		/** What opened it, which it sits beside. */
		anchor: Element;
		onclose: () => void;
	} = $props();

	type Step =
		| { kind: 'menu' }
		| { kind: 'checking' }
		| { kind: 'create' }
		| { kind: 'confirm'; playlist: Playlist; repeated: TrackSummary[] };
	let step = $state<Step>({ kind: 'menu' });
	let playlists = $state<Paged<Playlist> | null>(null);
	let failed = $state('');
	let menu = $state<HTMLElement>();
	let place = $state({ top: 0, left: 0 });
	let confirm = $state<HTMLDialogElement>();
	const libraryId = $derived(page.data.library?.id ?? '');
	const uid = $props.id();
	const confirmId = `already-${uid}`;
	const shown = $derived(playlists?.items.filter((p) => !deleted.has(p.id)) ?? []);

	onMount(() => {
		void position();
		items()[0]?.focus();
		void load();
		// It belongs beside what opened it, so it goes when that scrolls away, but not when its
		// own list scrolls.
		const scrolled = (e: Event) => {
			if (step.kind === 'menu' && !menu?.contains(e.target as Node)) onclose();
		};
		addEventListener('scroll', scrolled, true);
		return () => removeEventListener('scroll', scrolled, true);
	});

	async function load() {
		try {
			// Every playlist, a page at a time as the list scrolls.
			const first = await listPlaylists(fetch, libraryId);
			playlists = new Paged(first, (cursor) => listPlaylists(fetch, libraryId, { cursor }));
		} catch (e) {
			failed = e instanceof Error ? e.message : String(e);
		}
		await position();
	}

	/** Under its anchor, lined up on the right, or above it when there is no room below. */
	async function position() {
		await tick();
		if (!menu) return;
		const a = anchor.getBoundingClientRect();
		const { width, height } = menu.getBoundingClientRect();
		const below = a.bottom + 4;
		const top = below + height > innerHeight - 8 ? Math.max(8, a.top - 4 - height) : below;
		const left = Math.min(Math.max(8, a.right - width), innerWidth - width - 8);
		place = { top, left };
	}

	const items = () => [...(menu?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];

	/** Arrow keys move between items, and Escape or Tab closes, as in any menu. */
	function onkeydown(e: KeyboardEvent) {
		const all = items();
		const at = all.indexOf(document.activeElement as HTMLElement);
		const go = (i: number) => all[(i + all.length) % all.length]?.focus();
		if (e.key === 'ArrowDown') go(at + 1);
		else if (e.key === 'ArrowUp') go(at - 1);
		else if (e.key === 'Home') go(0);
		else if (e.key === 'End') go(all.length - 1);
		else if (e.key === 'Escape' || e.key === 'Tab') onclose();
		else return;
		e.preventDefault();
		e.stopPropagation();
	}

	function onpointerdown(e: PointerEvent) {
		const target = e.target as Node;
		if (step.kind === 'menu' && !menu?.contains(target) && !anchor.contains(target)) onclose();
	}

	async function choose(playlist: Playlist) {
		step = { kind: 'checking' };
		let repeated: string[];
		try {
			repeated = await alreadyIn(fetch, playlist, tracks);
		} catch (e) {
			toast(`Couldn’t add to “${playlist.title}”. ${e instanceof Error ? e.message : ''}`.trim());
			onclose();
			return;
		}
		if (repeated.length === 0) return add(playlist, tracks);
		step = {
			kind: 'confirm',
			playlist,
			repeated: tracks.filter((t) => repeated.includes(t.id))
		};
		await tick();
		confirm?.showModal();
	}

	function add(playlist: Playlist, which: TrackSummary[]) {
		void addTracks(fetch, playlist, which);
		onclose();
	}

	/**
	 * The last request and its `Idempotency-Key`. Sending the same request again reuses the key,
	 * so a retry after a lost response still makes one playlist; a changed one gets a new key.
	 */
	let sent: { request: string; key: string } | null = null;

	async function create({ title, description }: PlaylistDetails) {
		const body: PlaylistCreate = { title, items: playables(tracks) };
		if (description) body.description = description;
		const request = JSON.stringify(body);
		if (sent?.request !== request) sent = { request, key: crypto.randomUUID() };
		await createPlaylist(fetch, libraryId, body, sent.key);
		toast(`Added ${describe(tracks)} to “${title}”`);
		onclose();
	}
</script>

<svelte:window {onpointerdown} />

{#if step.kind === 'menu'}
	<div
		class="menu"
		role="menu"
		tabindex="-1"
		aria-label="Add {describe(tracks)} to a playlist"
		bind:this={menu}
		style:top="{place.top}px"
		style:left="{place.left}px"
		{onkeydown}
	>
		<p class="heading muted" aria-hidden="true">Add to playlist</p>
		<button type="button" role="menuitem" onclick={() => (step = { kind: 'create' })}>
			<Icon name="plus" size={16} /> New playlist…
		</button>
		{#if shown.length}<hr />{/if}
		<div class="list">
			{#each shown as playlist (playlist.id)}
				<button type="button" role="menuitem" onclick={() => choose(playlist)}>
					<span class="title">{playlist.title}</span>
					<span class="count muted">{plural(playlist.trackCount, 'song')}</span>
				</button>
			{/each}
			{#if playlists}
				<div {@attach (el) => playlists?.sentinel(el, el.parentElement)} aria-hidden="true"></div>
			{/if}
		</div>
		{#if playlists === null && !failed}
			<p class="state muted">Loading your playlists…</p>
		{:else if failed}
			<p class="state error">{failed}</p>
		{/if}
	</div>
{:else if step.kind === 'create'}
	<PlaylistDetailsDialog
		heading="New playlist"
		action="Create"
		busyAction="Creating…"
		onsave={create}
		{onclose}
	/>
{:else if step.kind === 'confirm'}
	{@const { playlist, repeated } = step}
	{@const fresh = tracks.filter((t) => !repeated.includes(t))}
	<dialog
		bind:this={confirm}
		aria-labelledby={confirmId}
		oncancel={(e) => {
			e.preventDefault();
			onclose();
		}}
	>
		<h2 id={confirmId}>Already in this playlist</h2>
		<p>
			{#if tracks.length === 1}
				{describe(tracks)} is already in “{playlist.title}”. Add it anyway?
			{:else if fresh.length === 0}
				These songs are already in “{playlist.title}”. Add them anyway?
			{:else}
				{repeated.length} of these {tracks.length} songs {repeated.length === 1 ? 'is' : 'are'}
				already in “{playlist.title}”. Add them anyway?
			{/if}
		</p>
		<footer>
			<button type="button" onclick={onclose}>Cancel</button>
			{#if fresh.length}
				<button type="button" onclick={() => add(playlist, fresh)}>Skip duplicates</button>
			{/if}
			<button type="button" class="primary" onclick={() => add(playlist, tracks)}>
				Add anyway
			</button>
		</footer>
	</dialog>
{/if}

<style>
	.menu {
		position: fixed;
		z-index: 40;
		display: flex;
		flex-direction: column;
		width: min(280px, calc(100vw - 16px));
		padding: var(--space-1);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		box-shadow: var(--shadow);
	}

	.heading {
		margin: 0;
		padding: var(--space-2) var(--space-3) var(--space-1);
		font-size: 12px;
		font-weight: 600;
	}

	.list {
		display: flex;
		flex-direction: column;
		max-height: min(320px, 50vh);
		overflow-y: auto;
		/* Its end does not scroll the page behind, which would close it. */
		overscroll-behavior: contain;
	}

	.menu button {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-height: 36px;
		padding: var(--space-1) var(--space-3);
		border: 0;
		border-radius: var(--radius-s);
		background: transparent;
		color: var(--text);
		text-align: left;
		cursor: pointer;
	}

	.menu button:hover,
	.menu button:focus-visible {
		background: var(--surface-hover);
		outline: none;
	}

	.title {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.count {
		flex-shrink: 0;
		font-size: 12px;
	}

	hr {
		width: 100%;
		margin: var(--space-1) 0;
		border: 0;
		border-top: 1px solid var(--border);
	}

	.state {
		margin: 0;
		padding: var(--space-2) var(--space-3);
	}

	.error {
		color: var(--danger);
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
</style>
