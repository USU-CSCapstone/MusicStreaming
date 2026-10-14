<script lang="ts">
	// Names a new playlist, which starts empty (`requirements/playlists.md` §2), and opens it.
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { createPlaylist } from '$lib/api/client';
	import type { PlaylistCreate } from '$lib/api/types';

	let { libraryId, onclose }: { libraryId: string; onclose: () => void } = $props();

	let dialog: HTMLDialogElement;
	const uid = $props.id();
	const titleId = `new-playlist-${uid}`;

	let title = $state('');
	let description = $state('');
	let error = $state('');
	let busy = $state(false);
	/**
	 * The last request and its `Idempotency-Key`. Sending the same request again reuses the key,
	 * so a retry after a lost response still makes one playlist; a changed one gets a new key.
	 */
	let sent: { request: string; key: string } | null = null;

	onMount(() => {
		// Focus starts on the title, the first field.
		dialog.showModal();
	});

	async function create(e: SubmitEvent) {
		e.preventDefault();
		const body: PlaylistCreate = { title: title.trim() };
		if (description.trim()) body.description = description.trim();
		const request = JSON.stringify(body);
		if (sent?.request !== request) sent = { request, key: crypto.randomUUID() };

		busy = true;
		error = '';
		try {
			const playlist = await createPlaylist(fetch, libraryId, body, sent.key);
			// The dialog stays up, busy, until the new playlist's page replaces this one.
			await goto(resolve('/playlists/[id]', { id: playlist.id }));
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
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
	<form onsubmit={create}>
		<h2 id={titleId}>New playlist</h2>

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
				>{busy ? 'Creating…' : 'Create'}</button
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
