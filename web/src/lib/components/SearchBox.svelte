<script lang="ts" module>
	/** Where clearing the box returns to: the page the user was on before searching. */
	let returnTo: string | null = null;
</script>

<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import Icon from './Icon.svelte';

	let input: HTMLInputElement;
	let text = $state(page.url.searchParams.get('q') ?? '');
	const searching = $derived(page.route.id === '/search');

	// Follow the URL, so leaving search clears the box and Back restores it. Not while
	// typing: a navigation finishing mid-word must not undo the keystrokes after it.
	$effect(() => {
		const q = searching ? (page.url.searchParams.get('q') ?? '') : '';
		if (document.activeElement !== input) text = q;
	});

	function oninput() {
		if (!text.trim()) return clear();
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

	function clear() {
		text = '';
		if (searching) {
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- a path this app was already on
			void goto(returnTo ?? resolve('/albums'), { replaceState: true, keepFocus: true });
		}
		returnTo = null;
	}
</script>

<div class="search">
	<Icon name="search" size={16} />
	<input
		bind:this={input}
		bind:value={text}
		{oninput}
		onkeydown={(e) => e.key === 'Escape' && clear()}
		type="search"
		placeholder="Search"
		aria-label="Search the library"
		autocomplete="off"
		spellcheck="false"
	/>
	{#if text}
		<button type="button" aria-label="Clear search" onclick={clear}>
			<Icon name="close" size={14} />
		</button>
	{/if}
</div>

<style>
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
		display: grid;
		place-items: center;
		width: 22px;
		height: 22px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
	}

	button:hover {
		background: var(--surface-hover);
		color: var(--text);
	}
</style>
