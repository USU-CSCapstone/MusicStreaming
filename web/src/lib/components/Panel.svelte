<script lang="ts">
	import type { Snippet } from 'svelte';
	import Icon from './Icon.svelte';

	// A panel over the main area, opened from the player: a title, a line under it, and what it
	// shows. Escape closes it.
	let {
		title,
		subtitle,
		onclose,
		children
	}: { title: string; subtitle?: string; onclose: () => void; children: Snippet } = $props();

	const id = $props.id();

	/** Escape closes it, unless something in front of it, such as a dialog, has it. */
	function onkeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape' || e.defaultPrevented || e.isComposing) return;
		if (document.querySelector('dialog[open]')) return;
		onclose();
	}
</script>

<svelte:window {onkeydown} />

<aside class="panel" aria-labelledby={id}>
	<header>
		<div class="meta">
			<h2 {id}>{title}</h2>
			{#if subtitle}
				<p class="muted">{subtitle}</p>
			{/if}
		</div>
		<button type="button" aria-label="Close {title.toLowerCase()}" onclick={onclose}>
			<Icon name="close" size={18} />
		</button>
	</header>
	<div class="body">
		{@render children()}
	</div>
</aside>

<style>
	.panel {
		position: absolute;
		inset: 0 0 0 auto;
		z-index: 10;
		display: flex;
		flex-direction: column;
		width: min(400px, 100%);
		background: var(--surface);
		border-left: 1px solid var(--border);
		box-shadow: var(--shadow);
	}

	header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-4) var(--space-3) var(--space-5);
		border-bottom: 1px solid var(--border);
	}

	.meta {
		min-width: 0;
	}

	h2 {
		font-size: 18px;
	}

	.meta p {
		margin: 2px 0 0;
		font-size: 13px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	header button {
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
		flex-shrink: 0;
	}

	header button:hover {
		background: var(--surface-hover);
		color: var(--text);
	}

	.body {
		flex: 1;
		overflow-y: auto;
		padding: var(--space-4) var(--space-5) var(--space-6);
	}
</style>
