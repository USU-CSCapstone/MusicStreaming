<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		submit,
		busyLabel,
		action,
		children
	}: {
		title: string;
		/** The button's label, and its label while `action` runs. */
		submit: string;
		busyLabel: string;
		/** Runs on submit; what it throws is shown as the form's error, so it must be safe to show. */
		action: () => Promise<void>;
		children: Snippet;
	} = $props();

	let error = $state('');
	let busy = $state(false);

	async function onsubmit(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			await action();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}
</script>

<!-- The form setup and login share, on its own before the app's shell appears. -->
<main>
	<form {onsubmit}>
		<h1>{title}</h1>
		{@render children()}
		{#if error}
			<p class="error" role="alert">{error}</p>
		{/if}
		<button type="submit" disabled={busy}>{busy ? busyLabel : submit}</button>
	</form>
</main>

<style>
	main {
		display: grid;
		place-items: center;
		min-height: 100dvh;
		padding: var(--space-4);
	}

	form {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		width: 100%;
		max-width: 380px;
		padding: var(--space-6);
		border-radius: var(--radius-l);
		background: var(--surface);
		box-shadow: var(--shadow);
	}

	h1 {
		font-size: 22px;
	}

	form :global(p) {
		margin: 0;
	}

	form :global(label) {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		font-weight: 500;
	}

	form :global(label .muted) {
		font-weight: 400;
	}

	form :global(input) {
		height: 36px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface-2);
		color: var(--text);
		font: inherit;
		font-weight: 400;
	}

	form :global(input:focus-visible) {
		outline-offset: -1px;
	}

	.error {
		color: var(--danger);
	}

	button {
		height: 40px;
		border: 0;
		border-radius: var(--radius-m);
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		cursor: pointer;
	}

	button:disabled {
		opacity: 0.6;
		cursor: default;
	}
</style>
