<script lang="ts">
	let { url }: { url: string } = $props();

	let link: HTMLElement;
	/** How the last press went, and for which link, so a new link does not inherit it. */
	let result = $state<{ url: string; how: 'yes' | 'select' } | null>(null);
	const copied = $derived(result?.url === url ? result.how : null);

	/** Copies the link; where the clipboard is out of reach, selects it for the user to copy. */
	async function copy() {
		try {
			await navigator.clipboard.writeText(url);
			result = { url, how: 'yes' };
		} catch {
			getSelection()?.selectAllChildren(link);
			result = { url, how: 'select' };
		}
	}
</script>

<div class="copy">
	<code bind:this={link}>{url}</code>
	<button type="button" onclick={copy}>Copy</button>
</div>
<p class="muted small" role="status">
	{#if copied === 'yes'}Copied.{:else if copied === 'select'}Selected. Copy it with your keyboard or
		menu.{/if}
</p>

<style>
	.copy {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	code {
		flex: 1;
		min-width: 0;
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		overflow-wrap: anywhere;
		user-select: all;
	}

	button {
		flex: none;
		height: 34px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 17px;
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		cursor: pointer;
	}

	p {
		margin: var(--space-1) 0 0;
	}

	.small {
		font-size: 13px;
	}
</style>
