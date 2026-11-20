<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		confirmLabel,
		danger = false,
		password = false,
		action,
		onclose,
		children
	}: {
		title: string;
		confirmLabel: string;
		/** Styles the confirm button as destructive. */
		danger?: boolean;
		/** Asks for the actor's own password, which `action` receives. */
		password?: boolean;
		/** What confirming does; a throw keeps the dialog open and shows its message. */
		action: (password: string) => Promise<void>;
		/** Called when the dialog is cancelled, and after `action` succeeds: the caller removes it. */
		onclose: () => void;
		children?: Snippet;
	} = $props();

	let dialog: HTMLDialogElement;
	let typed = $state('');
	let busy = $state(false);
	let error = $state<string | null>(null);
	const uid = $props.id();
	const titleId = `confirm-${uid}`;

	$effect(() => {
		dialog.showModal();
		return () => dialog.close();
	});

	async function confirm(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = null;
		try {
			await action(typed);
			onclose();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Something went wrong. Try again.';
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	bind:this={dialog}
	aria-labelledby={titleId}
	oncancel={(e) => {
		e.preventDefault();
		if (!busy) onclose();
	}}
>
	<form onsubmit={confirm}>
		<h2 id={titleId}>{title}</h2>
		{@render children?.()}
		{#if password}
			<label class="password">
				Your password
				<input
					type="password"
					autocomplete="current-password"
					required
					bind:value={typed}
					disabled={busy}
				/>
			</label>
		{/if}
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<div class="actions">
			<button type="button" class="cancel" disabled={busy} onclick={onclose}>Cancel</button>
			<button type="submit" class:danger disabled={busy}>{confirmLabel}</button>
		</div>
	</form>
</dialog>

<style>
	dialog {
		width: min(440px, calc(100vw - 32px));
		max-height: calc(100dvh - 32px);
		padding: 0;
		border: 1px solid var(--border);
		border-radius: var(--radius-l);
		background: var(--surface);
		color: var(--text);
		box-shadow: 0 20px 60px #00000040;
	}

	dialog::backdrop {
		background: #00000066;
	}

	form {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding: var(--space-5);
	}

	h2 {
		margin: 0;
		font-size: 18px;
	}

	form :global(p) {
		margin: 0;
	}

	.password {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}

	input {
		height: 40px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font: inherit;
	}

	.actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-3);
	}

	button {
		min-height: 40px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 20px;
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		cursor: pointer;
	}

	.cancel {
		background: var(--surface-2);
		color: inherit;
	}

	.danger {
		background: #dc2626;
		color: #fff;
	}

	button:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.error {
		color: #dc2626;
	}

	@media (prefers-color-scheme: dark) {
		.error {
			color: #f87171;
		}
	}
</style>
