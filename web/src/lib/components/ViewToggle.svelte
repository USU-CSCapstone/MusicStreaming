<script lang="ts">
	import { prefs, type CollectionView } from '$lib/prefs.svelte';
	import Icon from './Icon.svelte';

	const options: { value: CollectionView; label: string }[] = [
		{ value: 'grid', label: 'Grid view' },
		{ value: 'list', label: 'List view' }
	];
</script>

<div class="toggle" role="group" aria-label="View">
	{#each options as o (o.value)}
		<button
			type="button"
			aria-label={o.label}
			aria-pressed={prefs.view === o.value}
			title={o.label}
			onclick={() => (prefs.view = o.value)}
		>
			<Icon name={o.value} size={18} />
		</button>
	{/each}
</div>

<style>
	.toggle {
		display: inline-flex;
		padding: 2px;
		gap: 2px;
		border-radius: var(--radius-m);
		background: var(--surface-2);
	}

	button {
		display: grid;
		place-items: center;
		width: 34px;
		height: 30px;
		border: 0;
		border-radius: calc(var(--radius-m) - 2px);
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
	}

	button:hover {
		color: var(--text);
	}

	button[aria-pressed='true'] {
		background: var(--surface);
		color: var(--text);
		box-shadow: var(--shadow);
	}
</style>
