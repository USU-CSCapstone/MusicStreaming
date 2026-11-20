<script lang="ts">
	// All, then a tab for each type of result, each a link to the same search on it.
	import { SEARCH_TABS, tabHref, type SearchTab } from '$lib/search-tabs';

	let { q, current }: { q: string; current: SearchTab | null } = $props();

	const tabs = $derived([{ type: null, label: 'All' }, ...SEARCH_TABS]);

	/** At phone width the tabs scroll sideways, so the one open is brought into view. */
	const shown = (el: HTMLElement) => el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
</script>

<nav class="tabs" aria-label="Search results">
	{#each tabs as tab (tab.type)}
		<!-- eslint-disable svelte/no-navigation-without-resolve -- resolved in search-tabs.ts -->
		<a
			href={tabHref(q, tab.type)}
			class:active={tab.type === current}
			aria-current={tab.type === current ? 'page' : undefined}
			{@attach tab.type === current && shown}
			data-sveltekit-noscroll
			data-sveltekit-keepfocus>{tab.label}</a
		>
		<!-- eslint-enable svelte/no-navigation-without-resolve -->
	{/each}
</nav>

<style>
	.tabs {
		display: flex;
		gap: var(--space-2);
		margin-bottom: var(--space-5);
		/* At phone width the tabs scroll sideways rather than wrapping. */
		overflow-x: auto;
		scrollbar-width: none;
	}

	a {
		display: inline-flex;
		flex-shrink: 0;
		align-items: center;
		min-height: 36px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: 18px;
		background: var(--surface);
		color: var(--text);
		font-weight: 500;
		text-decoration: none;
	}

	a:hover {
		background: var(--surface-hover);
	}

	a.active {
		border-color: transparent;
		background: var(--text);
		color: var(--bg);
	}
</style>
