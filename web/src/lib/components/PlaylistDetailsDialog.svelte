<script lang="ts" module>
	export type PlaylistDetails = { title: string; description: string | null };
</script>

<script lang="ts">
	// A playlist's title and description (`requirements/playlists.md` §2), for a new one or an
	// existing one. Saving is the caller's: a failure it throws shows here, and the dialog stays.
	import { onMount } from 'svelte';

	let {
		heading,
		action,
		busyAction,
		initial = { title: '', description: null },
		onsave,
		onclose
	}: {
		heading: string;
		/** The submit button: "Create", "Save". */
		action: string;
		/** The submit button while saving: "Creating…". */
		busyAction: string;
		initial?: PlaylistDetails;
		/** Trimmed, with an empty description as null. */
		onsave: (details: PlaylistDetails) => unknown;
		onclose: () => void;
	} = $props();

	let dialog: HTMLDialogElement;
	const uid = $props.id();
	const titleId = `playlist-details-${uid}`;

	// The form starts from the details as they were when it opened.
	// svelte-ignore state_referenced_locally
	let title = $state(initial.title);
	// svelte-ignore state_referenced_locally
	let description = $state(initial.description ?? '');
	let error = $state('');
	let busy = $state(false);

	onMount(() => {
		// Focus starts on the title, the first field.
		dialog.showModal();
	});

	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			await onsave({ title: title.trim(), description: description.trim() || null });
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
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
	<form onsubmit={save}>
		<h2 id={titleId}>{heading}</h2>

		<label>
			Title
			<input bind:value={title} required disabled={busy} />
		</label>
		<label>
			<span>Description <span class="muted">(optional)</span></span>
			<textarea bind:value={description} rows="3" disabled={busy}></textarea>
		</label>

		{#if error}<p class="error" role="alert">{error}</p>{/if}

		<footer>
			<button type="button" onclick={onclose} disabled={busy}>Cancel</button>
			<button type="submit" class="primary" disabled={busy || !title.trim()}
				>{busy ? busyAction : action}</button
			>
		</footer>
	</form>
</dialog>

<style>
	dialog {
		width: min(440px, calc(100vw - 2 * var(--space-4)));
		padding: 0;
		border: 0;
		border-radius: var(--radius-l);
		background: var(--surface);
		color: var(--text);
		box-shadow: var(--shadow);
	}

	dialog::backdrop {
		background: #0006;
	}

	form {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding: var(--space-5);
	}

	h2 {
		font-size: 18px;
	}

	p {
		margin: 0;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		font-weight: 500;
	}

	input,
	textarea {
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface-2);
		color: var(--text);
		font: inherit;
		font-weight: 400;
	}

	input {
		height: 36px;
	}

	textarea {
		resize: vertical;
	}

	.error {
		color: var(--danger);
	}

	footer {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-2);
		margin-top: var(--space-2);
	}

	button {
		height: 36px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font: inherit;
		font-weight: 500;
		cursor: pointer;
	}

	button.primary {
		border-color: var(--accent);
		background: var(--accent);
		color: var(--accent-text);
	}

	button:disabled {
		opacity: 0.6;
		cursor: default;
	}
</style>
