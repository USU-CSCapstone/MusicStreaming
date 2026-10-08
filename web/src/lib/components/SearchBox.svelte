<script lang="ts" module>
	import { recordRecentSearch } from '$lib/api/client';
	import { SearchSettler } from '$lib/recent';

	/** Where clearing the box returns to: the page the user was on before searching. */
	let returnTo: string | null = null;

	/** When the search on screen has settled enough to keep as a recent search. */
	export const settler = new SearchSettler((libraryId, query, selected, keepalive) => {
		// A recent search that fails to record is only a convenience lost.
		recordRecentSearch(fetch, libraryId, query, selected, keepalive).catch(() => {});
	});
</script>

<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { clearRecentSearches, deleteRecentSearch, listRecentSearches } from '$lib/api/client';
	import type { RecentSearch } from '$lib/api/types';
	import Icon from './Icon.svelte';

	let input: HTMLInputElement;
	let list = $state<HTMLElement>();
	let text = $state(page.url.searchParams.get('q') ?? '');
	const searching = $derived(page.route.id === '/search');
	const libraryId = $derived(page.data.library?.id ?? '');

	// Recent searches are offered while the box is focused and empty (`requirements/search.md` §6).
	let focused = $state(false);
	let recent = $state<RecentSearch[]>([]);
	const offering = $derived(focused && !text && recent.length > 0);

	// Follow the URL, so leaving search clears the box and Back restores it. Not while
	// typing: a navigation finishing mid-word must not undo the keystrokes after it.
	$effect(() => {
		const q = searching ? (page.url.searchParams.get('q') ?? '') : '';
		if (document.activeElement !== input) text = q;
	});

	function search() {
		if (!searching) returnTo = page.url.pathname + page.url.search;
		// Results update as the user types, with no submit step (`requirements/search.md` §7).
		// Each keystroke replaces the entry, so Back leaves search in one step.
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolve() plus a query string
		void goto(`${resolve('/search')}?${new URLSearchParams({ q: text })}`, {
			replaceState: searching,
			keepFocus: true,
			noScroll: true
		});
	}

	function oninput() {
		if (text.trim()) search();
		else clear();
	}

	function clear() {
		text = '';
		if (searching) {
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- a path this app was already on
			void goto(returnTo ?? resolve('/albums'), { replaceState: true, keepFocus: true });
		}
		returnTo = null;
		if (focused) void load();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && text.trim()) settler.settle();
		else if (e.key === 'Escape') {
			if (text) clear();
			else input.blur();
		} else if (e.key === 'ArrowDown' && offering) {
			e.preventDefault();
			list?.querySelector<HTMLElement>('.query')?.focus();
		}
	}

	/** Up and down move between the offered searches, and up from the first returns to the box. */
	function onlistkeydown(e: KeyboardEvent) {
		if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
		e.preventDefault();
		const queries = [...(list?.querySelectorAll<HTMLElement>('.query') ?? [])];
		const at = queries.indexOf(document.activeElement as HTMLElement);
		const next = at + (e.key === 'ArrowDown' ? 1 : -1);
		if (next < 0) input.focus();
		else queries[Math.min(next, queries.length - 1)]?.focus();
	}

	async function load() {
		if (!libraryId) return;
		try {
			recent = (await listRecentSearches(fetch, libraryId)).items;
		} catch {
			recent = [];
		}
	}

	function onfocusin() {
		if (focused) return;
		focused = true;
		if (!text) void load();
	}

	function onfocusout(e: FocusEvent) {
		const within = (e.currentTarget as Element).contains(e.relatedTarget as Node | null);
		if (!within) focused = false;
	}

	function choose(r: RecentSearch) {
		text = r.query;
		input.focus();
		search();
	}

	// Removing applies at once and is confirmed in the background (`requirements/general.md` §3.4).
	function remove(r: RecentSearch) {
		recent = recent.filter((other) => other.id !== r.id);
		input.focus();
		deleteRecentSearch(fetch, libraryId, r.id).catch(load);
	}

	function clearAll() {
		recent = [];
		input.focus();
		clearRecentSearches(fetch, libraryId).catch(load);
	}
</script>

<div class="wrap" {onfocusin} {onfocusout}>
	<div class="search">
		<Icon name="search" size={16} />
		<input
			bind:this={input}
			bind:value={text}
			{oninput}
			{onkeydown}
			type="search"
			placeholder="Search"
			aria-label="Search the library"
			autocomplete="off"
			spellcheck="false"
		/>
		{#if text}
			<button type="button" class="icon" aria-label="Clear search" onclick={clear}>
				<Icon name="close" size={14} />
			</button>
		{/if}
	</div>
	{#if offering}
		<!-- Pressing here must not take focus from the box first, or the list would close
		     before the press lands; arrow keys move between the searches. Every action is a
		     button, so neither listener is the only way to reach one. -->
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<div
			class="recent"
			role="region"
			aria-label="Recent searches"
			bind:this={list}
			onmousedown={(e) => e.preventDefault()}
			onkeydown={onlistkeydown}
		>
			<header>
				<span>Recent searches</span>
				<button type="button" class="clear-all" onclick={clearAll}>Clear all</button>
			</header>
			<ul>
				{#each recent as r (r.id)}
					<li>
						<button type="button" class="query" onclick={() => choose(r)}>{r.query}</button>
						<button
							type="button"
							class="icon"
							aria-label="Remove “{r.query}” from recent searches"
							onclick={() => remove(r)}
						>
							<Icon name="close" size={12} />
						</button>
					</li>
				{/each}
			</ul>
		</div>
	{/if}
</div>

<style>
	.wrap {
		position: relative;
	}

	.search {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		height: 36px;
		padding: 0 var(--space-2) 0 var(--space-3);
		border-radius: var(--radius-m);
		background: var(--surface-2);
		color: var(--text-muted);
	}

	.search:focus-within {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}

	input {
		flex: 1;
		min-width: 0;
		border: 0;
		outline: 0;
		background: transparent;
		color: var(--text);
		font: inherit;
	}

	input::-webkit-search-cancel-button {
		display: none;
	}

	button {
		border: 0;
		background: transparent;
		font: inherit;
		cursor: pointer;
	}

	.icon {
		display: grid;
		flex: none;
		place-items: center;
		width: 22px;
		height: 22px;
		border-radius: 50%;
		color: var(--text-muted);
	}

	.icon:hover {
		background: var(--surface-hover);
		color: var(--text);
	}

	.recent {
		position: absolute;
		top: calc(100% + var(--space-1));
		left: 0;
		right: 0;
		z-index: 10;
		max-height: min(60vh, 420px);
		overflow-y: auto;
		padding: var(--space-1);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		box-shadow: var(--shadow);
	}

	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: var(--space-1) var(--space-2);
		color: var(--text-muted);
		font-size: 12px;
	}

	.clear-all {
		padding: 2px var(--space-1);
		border-radius: var(--radius-s);
		color: var(--accent);
		font-size: 12px;
	}

	.clear-all:hover {
		background: var(--accent-soft);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	li {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		padding-right: var(--space-1);
		border-radius: var(--radius-s);
	}

	li:hover,
	li:focus-within {
		background: var(--surface-hover);
	}

	.query {
		flex: 1;
		min-width: 0;
		/* A comfortable target for touch. */
		min-height: 36px;
		padding: 0 var(--space-2);
		overflow: hidden;
		color: var(--text);
		text-align: left;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.query:focus-visible,
	.clear-all:focus-visible,
	.icon:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
</style>
