<script lang="ts">
	// All, then a tab for each type of result and each type of tag, each a link to the same search
	// on it.
	import { page } from '$app/state';
	import { sameTab, searchTabs, tabHref, type SearchTab, type Tab } from '$lib/search-tabs';

	let {
		q,
		current,
		scope = {},
		holds
	}: {
		q: string;
		current: Tab | null;
		/** The parameters of the container offered or kept to, which every tab keeps. */
		scope?: Record<string, string>;
		/** The types of result there can be, where a container holds only some. */
		holds?: SearchTab[];
	} = $props();

	const tabs = $derived(
		searchTabs(page.data.tagTypes ?? []).filter(({ tab }) => !tab || !holds || holds.includes(tab.type))
	);

	/** At phone width the tabs scroll sideways, so the one open is brought into view. */
	const shown = (el: HTMLElement) => el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
</script>

<nav class="tabs" aria-label="Search results">
	{#each tabs as { tab, label } (`${tab?.type}:${tab?.tagType}`)}
		{@const open = sameTab(tab, current)}
		<!-- eslint-disable svelte/no-navigation-without-resolve -- resolved in search-tabs.ts -->
		<a
			href={tabHref(q, tab, scope)}
			class:active={open}
			aria-current={open ? 'page' : undefined}
			{@attach open && shown}
			data-sveltekit-noscroll
			data-sveltekit-keepfocus>{label}</a
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
