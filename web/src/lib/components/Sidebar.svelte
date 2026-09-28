<script lang="ts">
	import { page } from '$app/state';
	import { plural } from '$lib/format';
	import { inSection, sections } from '$lib/sections';
	import Icon from './Icon.svelte';
	import SearchBox from './SearchBox.svelte';

	const library = $derived(page.data.library);
</script>

<nav class="sidebar" aria-label="Library">
	<SearchBox />
	<ul>
		{#each sections as s (s.route)}
			{@const active = inSection(page.route.id, s)}
			<li>
				<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- resolved in sections.ts -->
				<a href={s.href} class:active aria-current={active ? 'page' : undefined}>
					<Icon name={s.icon} size={18} />
					{s.label}
				</a>
			</li>
		{/each}
	</ul>
	{#if library}
		<footer class="muted">
			<span class="name">{library.name}</span>
			<span>{plural(library.trackCount, 'song')}{library.scanning ? ' · scanning' : ''}</span>
		</footer>
	{/if}
</nav>

<style>
	.sidebar {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding: var(--space-4) var(--space-3);
		background: var(--surface);
		border-right: 1px solid var(--border);
		overflow-y: auto;
	}

	ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	a {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		height: 36px;
		padding: 0 var(--space-3);
		border-radius: var(--radius-m);
		color: var(--text-muted);
		font-weight: 500;
	}

	a:hover {
		background: var(--surface-hover);
		color: var(--text);
		text-decoration: none;
	}

	a.active {
		background: var(--accent-soft);
		color: var(--accent);
	}

	footer {
		margin-top: auto;
		display: flex;
		flex-direction: column;
		padding: 0 var(--space-3);
		font-size: 12px;
	}

	.name {
		font-weight: 600;
	}
</style>
