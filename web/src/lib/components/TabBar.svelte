<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { inSection, sections } from '$lib/sections';
	import Icon from './Icon.svelte';

	const tabs = [
		...sections,
		{ href: resolve('/search'), route: '/search', label: 'Search', icon: 'search' as const }
	];
</script>

<nav class="tabs" aria-label="Library">
	{#each tabs as t (t.route)}
		{@const active = inSection(page.route.id, t)}
		<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved in sections.ts -->
		<a href={t.href} class:active aria-current={active ? 'page' : undefined}>
			<Icon name={t.icon} size={22} />
			<span>{t.label}</span>
		</a>
	{/each}
</nav>

<style>
	.tabs {
		display: grid;
		grid-template-columns: repeat(5, 1fr);
		padding-bottom: env(safe-area-inset-bottom);
		background: var(--surface);
		border-top: 1px solid var(--border);
	}

	a {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 2px;
		height: 56px;
		color: var(--text-muted);
		font-size: 11px;
		font-weight: 500;
	}

	a:hover {
		text-decoration: none;
	}

	a.active {
		color: var(--accent);
	}
</style>
