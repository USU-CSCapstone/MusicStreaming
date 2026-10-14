<script lang="ts">
	// Where selected songs go, as a position: a way to reorder without dragging, on any device
	// (`requirements/playlists.md` §4).
	import { onMount } from 'svelte';
	import { plural } from '$lib/format';

	let {
		count,
		last,
		start,
		onmove,
		onclose
	}: {
		/** How many songs are moving. */
		count: number;
		/** The lowest position they can take, counting from 1: where they end up at the bottom. */
		last: number;
		/** Where the first of them is now, counting from 1. */
		start: number;
		/** The position the first of them takes, counting from 0. */
		onmove: (to: number) => void;
		onclose: () => void;
	} = $props();

	let dialog: HTMLDialogElement;
	const uid = $props.id();
	const titleId = `move-to-${uid}`;
	// svelte-ignore state_referenced_locally
	let position = $state(start);
	const valid = $derived(Number.isInteger(position) && position >= 1 && position <= last);

	onMount(() => {
		dialog.showModal();
	});

	function move(to: number) {
		onmove(to - 1);
		onclose();
	}
</script>

<dialog
	bind:this={dialog}
	aria-labelledby={titleId}
	oncancel={(e) => {
		e.preventDefault();
		onclose();
	}}
>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			if (valid) move(position);
		}}
	>
		<h2 id={titleId}>Move {plural(count, 'song')}</h2>

		<div class="ends">
			<button type="button" onclick={() => move(1)}>To the top</button>
			<button type="button" onclick={() => move(last)}>To the bottom</button>
		</div>
		<label>
			<span>Position <span class="muted">(1 to {last})</span></span>
			<input type="number" min="1" max={last} step="1" bind:value={position} required />
		</label>

		<footer>
			<button type="button" onclick={onclose}>Cancel</button>
			<button type="submit" class="primary" disabled={!valid}>Move</button>
		</footer>
	</form>
</dialog>

<style>
	dialog {
		width: min(360px, calc(100vw - 2 * var(--space-4)));
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

	.ends {
		display: flex;
		gap: var(--space-2);
	}

	.ends button {
		flex: 1;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		font-weight: 500;
	}

	input {
		height: 36px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface-2);
		color: var(--text);
		font: inherit;
		font-weight: 400;
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
