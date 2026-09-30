<script lang="ts">
	import '../app.css';
	import { page } from '$app/state';
	import favicon from '$lib/assets/favicon.svg';
	import Player from '$lib/components/Player.svelte';
	import Sidebar from '$lib/components/Sidebar.svelte';
	import TabBar from '$lib/components/TabBar.svelte';
	import { player } from '$lib/player.svelte';

	let { data, children } = $props();

	// Setup comes before there is anything to browse or play.
	const bare = $derived(page.route.id === '/setup');

	$effect.pre(() => {
		player.libraryId = data.library?.id ?? '';
	});

	/** Space plays and pauses, unless focus is somewhere Space already means something. */
	function onkeydown(e: KeyboardEvent) {
		if (e.key !== ' ' || e.repeat || e.defaultPrevented) return;
		const target = e.target as HTMLElement;
		if (target.closest('input, textarea, select, button, a, [role="slider"], [contenteditable]'))
			return;
		e.preventDefault();
		player.toggle();
	}
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<title>Jewelcase</title>
</svelte:head>

<svelte:window {onkeydown} />

{#if bare}
	{@render children()}
{:else}
	<div class="app">
		<div class="sidebar"><Sidebar /></div>
		<main>
			{@render children()}
		</main>
		<div class="player"><Player /></div>
		<div class="tabs"><TabBar /></div>
	</div>
{/if}

<style>
	.app {
		display: grid;
		grid-template-columns: var(--sidebar-width) minmax(0, 1fr);
		grid-template-rows: minmax(0, 1fr) auto;
		grid-template-areas:
			'sidebar main'
			'player player';
		height: 100dvh;
	}

	.sidebar {
		grid-area: sidebar;
		display: grid;
		min-height: 0;
	}

	main {
		grid-area: main;
		overflow-y: auto;
		padding: var(--space-5) var(--space-6) var(--space-6);
	}

	.player {
		grid-area: player;
	}

	.tabs {
		display: none;
	}

	/* Phone width: sections move to a tab bar under a compact player. */
	@media (max-width: 767px) {
		.app {
			grid-template-columns: minmax(0, 1fr);
			grid-template-rows: minmax(0, 1fr) auto auto;
			grid-template-areas:
				'main'
				'player'
				'tabs';
		}

		.sidebar {
			display: none;
		}

		main {
			padding: var(--space-4);
		}

		.tabs {
			display: block;
			grid-area: tabs;
		}
	}
</style>
