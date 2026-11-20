<script lang="ts">
	import { untrack } from 'svelte';
	import { ApiError, adminSetUserLibraries } from '$lib/api/client';
	import type { AdminUser, Library } from '$lib/api/types';

	let {
		row,
		libraries,
		onsaved
	}: {
		row: AdminUser;
		libraries: Library[];
		/** Called with the account as the server now has it. */
		onsaved: (user: AdminUser) => void;
	} = $props();

	// The checkboxes start from the grants the row has; saving replaces them with the boxes ticked.
	let chosen = $state<string[]>(untrack(() => [...row.libraryIds]));
	let saving = $state(false);
	let error = $state<string | null>(null);

	async function save(e: SubmitEvent) {
		e.preventDefault();
		saving = true;
		error = null;
		try {
			onsaved(await adminSetUserLibraries(fetch, row.id, chosen));
		} catch (err) {
			error = err instanceof ApiError ? err.message : 'Something went wrong. Try again.';
		} finally {
			saving = false;
		}
	}
</script>

<form onsubmit={save}>
	<fieldset disabled={saving}>
		<legend>Libraries {row.username} can reach</legend>
		{#if libraries.length === 0}
			<p class="muted">No libraries yet.</p>
		{/if}
		{#each libraries as library (library.id)}
			<label class="check">
				<input type="checkbox" value={library.id} bind:group={chosen} />
				{library.name}
			</label>
		{/each}
	</fieldset>
	<button type="submit" disabled={saving}>{saving ? 'Saving…' : 'Save libraries'}</button>
	{#if error}<p class="error" role="alert">{error}</p>{/if}
</form>

<style>
	form {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-3);
	}

	fieldset {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin: 0;
		padding: 0;
		border: 0;
	}

	legend {
		margin-bottom: var(--space-2);
		font-weight: 600;
	}

	.check {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		min-height: 32px;
	}

	p {
		margin: 0;
	}

	button {
		min-height: 36px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 18px;
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		cursor: pointer;
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
