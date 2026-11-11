<script lang="ts">
	import '../app.css';
	import { untrack } from 'svelte';
	import { page } from '$app/state';
	import favicon from '$lib/assets/favicon.svg';
	import DevicesPanel from '$lib/components/DevicesPanel.svelte';
	import LyricsPanel from '$lib/components/LyricsPanel.svelte';
	import QueuePanel from '$lib/components/QueuePanel.svelte';
	import Player from '$lib/components/Player.svelte';
	import Sidebar from '$lib/components/Sidebar.svelte';
	import TabBar from '$lib/components/TabBar.svelte';
	import Toast from '$lib/components/Toast.svelte';
	import { catalog } from '$lib/catalog/client.svelte';
	import { connectivity } from '$lib/connectivity.svelte';
	import { controls } from '$lib/controls.svelte';
	import { tellServiceWorker } from '$lib/image-cache';
	import { marks } from '$lib/marks.svelte';
	import { player } from '$lib/player.svelte';
	import { queue } from '$lib/queue/client.svelte';
	import { sender } from '$lib/sync/sender';
	import { checkForUpdate } from '$lib/app-update';
	import { MAX_RETRY_MS, realtime } from '$lib/realtime.svelte';

	let { data, children } = $props();

	// Setup and login come before there is anything to browse or play.
	const bare = $derived(page.route.id === '/setup' || page.route.id === '/login');

	$effect.pre(() => {
		player.libraryId = data.library?.id ?? '';
	});

	// Live updates, the account's queue and the device's copy of its libraries, for someone signed
	// in; setup and login have nothing to update. They leave the catalog's election but keep its
	// files, which stay the last account's until the server ends its session or another signs in.
	// Keyed on the account alone: a load run again, as `invalidateAll` does, gives the same
	// account as a new object, which must not start the catalog's worker over. Nothing the
	// connecting reads is tracked either: the workers read Work offline as they start, and a toggle
	// must reach them as `setWorkingOffline` (below), never as a restart.
	const userId = $derived(data.me?.id);
	$effect(() => {
		const user = userId;
		if (bare || !user) {
			untrack(() => catalog.disconnect());
			return;
		}
		const unsubscribe = untrack(() => {
			void tellServiceWorker(user);
			// A restarted server may be a new version: its connection saying hello is the cue to
			// look, and to send what was made while it could not be reached.
			const unsubscribe = realtime.on((message) => {
				if (message.type !== 'hello') return;
				checkForUpdate();
				sender.wake();
			});
			queue.connect(realtime, user, marks);
			catalog.connect(realtime, user);
			return unsubscribe;
		});
		return () => {
			unsubscribe();
			catalog.disconnect();
			queue.disconnect();
		};
	});

	// What was done while the server could not be reached waits in the log, kept for this account
	// alone, until it can be sent (`$lib/sync/sender`).
	$effect(() => {
		if (bare || !userId) return;
		sender.start(userId);
		return () => sender.stop();
	});

	// Offline, search is the device's: the library on screen has its index built ahead, in the
	// background, rather than on the first search. Online, none is built, which the server's search
	// makes needless. A lost connection counts once it has stayed lost for the reconnect's longest
	// wait, so a server restart costs an online device nothing; no network, or working offline,
	// counts at once. Only a change of that wait starts it again, so a network that comes and goes
	// still counts.
	const indexAfter = $derived(
		connectivity.state === 'online'
			? undefined
			: connectivity.state === 'unreachable'
				? MAX_RETRY_MS
				: 0
	);
	$effect(() => {
		const library = data.library?.id;
		const wait = indexAfter;
		if (bare || !data.me || !library || wait === undefined) return;
		const timer = setTimeout(() => catalog.prepareIndex(library), wait);
		return () => clearTimeout(timer);
	});

	// Working offline closes the realtime connection and keeps the workers from any request;
	// turning it off connects again, and the `hello` that follows makes them catch up.
	$effect(() => {
		const working = connectivity.workOffline;
		queue.setWorkingOffline(working);
		catalog.setWorkingOffline(working);
		if (working) player.stopStreaming();
		if (bare || !data.me || working) return;
		realtime.start();
		return () => realtime.stop();
	});

	/**
	 * Space plays and pauses what the player bar shows, here or on another device, unless focus
	 * is somewhere Space already means something.
	 */
	function onkeydown(e: KeyboardEvent) {
		if (e.key !== ' ' || e.repeat || e.defaultPrevented) return;
		const target = e.target as HTMLElement;
		if (target.closest('input, textarea, select, button, a, [role="slider"], [contenteditable]'))
			return;
		e.preventDefault();
		controls.toggle();
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
		<div class="main">
			<main>
				{@render children()}
			</main>
			<LyricsPanel />
			<QueuePanel />
			<DevicesPanel />
			<Toast />
		</div>
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

	.main {
		grid-area: main;
		position: relative;
		min-height: 0;
		display: grid;
	}

	main {
		/* Named so what sticks to the top of the page can cover it (TrackList's header). */
		--page-top: var(--space-5);
		/* What is positioned inside the page stays inside what scrolls. Without this, a
		   visually hidden live region at the end of a long list sits that far down the
		   window, and the whole app scrolls past the end of the list. */
		position: relative;
		overflow-y: auto;
		padding: var(--page-top) var(--space-6) var(--space-6);
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
			--page-top: var(--space-4);
			padding: var(--page-top) var(--space-4) var(--space-4);
		}

		.tabs {
			display: block;
			grid-area: tabs;
		}
	}
</style>
