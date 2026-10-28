<script lang="ts">
	// ⋯ beside a detail page's Play, and on a card, which opens the actions for the whole album,
	// artist, or playlist: queue it, or add it to a playlist.
	import type { Adding } from '$lib/adding.svelte';
	import AddToPlaylistMenu from './AddToPlaylistMenu.svelte';
	import Icon from './Icon.svelte';

	let { what }: { what: Adding } = $props();

	let button = $state<HTMLButtonElement>();
	let open = $state(false);

	/** Focus goes back to the button. */
	function close() {
		open = false;
		button?.focus();
	}
</script>

<button
	bind:this={button}
	type="button"
	class:open
	aria-label="Actions for {what.label}"
	title="More actions"
	aria-haspopup="menu"
	aria-expanded={open}
	onclick={() => (open ? close() : (open = true))}
>
	<Icon name="more" size={18} />
</button>
{#if open && button}
	<AddToPlaylistMenu {what} anchor={button} onclose={close} />
{/if}

<style>
	button {
		display: grid;
		place-items: center;
		width: 38px;
		height: 38px;
		border: 1px solid var(--border);
		border-radius: 50%;
		background: var(--surface);
		color: var(--text);
		cursor: pointer;
	}

	button:hover,
	button.open {
		background: var(--surface-hover);
	}
</style>
