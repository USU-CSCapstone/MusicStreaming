<script lang="ts">
	// Everywhere, or in the album, artist, playlist or tag the search was started from: the
	// choice made from the results, not from a search field on each page
	// (`requirements/search.md` §5). Each is a link to the same search, so Back undoes it.
	import { HELD, scopeParams, type Container } from '$lib/search-scope';
	import { tabHref, type Tab } from '$lib/search-tabs';

	let {
		q,
		tab,
		container,
		within
	}: { q: string; tab: Tab | null; container: Container; within: boolean } = $props();

	// Kept to the container, a tab of what it cannot hold would always be empty, so it is All.
	const held = $derived(tab && HELD[container.kind].includes(tab.type) ? tab : null);
	const options = $derived([
		{ label: 'Everywhere', scoped: false, href: tabHref(q, tab, scopeParams(container.within, false)) },
		{
			label: `In ${container.name}`,
			scoped: true,
			href: tabHref(q, held, scopeParams(container.within, true))
		}
	]);
</script>

<nav class="scope" aria-label="Where to search">
	{#each options as option (option.scoped)}
		<!-- eslint-disable svelte/no-navigation-without-resolve -- resolved in search-tabs.ts -->
		<a
			href={option.href}
			class:active={option.scoped === within}
			aria-current={option.scoped === within ? 'page' : undefined}
			data-sveltekit-noscroll
			data-sveltekit-keepfocus
		>
			<span>{option.label}</span>
		</a>
		<!-- eslint-enable svelte/no-navigation-without-resolve -->
	{/each}
</nav>

<style>
	.scope {
		display: inline-flex;
		max-width: 100%;
		margin-bottom: var(--space-4);
		padding: 3px;
		border-radius: 18px;
		background: var(--surface-2);
	}

	a {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		min-width: 0;
		min-height: 30px;
		padding: 0 var(--space-3);
		border-radius: 15px;
		color: var(--text-muted);
		font-weight: 500;
		text-decoration: none;
	}

	a span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	a:hover {
		color: var(--text);
	}

	a.active {
		background: var(--surface);
		box-shadow: var(--shadow);
		color: var(--text);
	}
</style>
