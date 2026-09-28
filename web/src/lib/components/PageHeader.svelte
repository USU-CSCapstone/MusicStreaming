<script lang="ts">
	import ViewToggle from './ViewToggle.svelte';

	let {
		title,
		subtitle,
		toggle = false
	}: { title: string; subtitle?: string; toggle?: boolean } = $props();
</script>

<header>
	<div>
		<h1>{title}</h1>
		{#if subtitle}<p class="muted">{subtitle}</p>{/if}
	</div>
	{#if toggle}<ViewToggle />{/if}
</header>

<style>
	header {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--space-4);
		margin-bottom: var(--space-5);
	}

	h1 {
		font-size: 28px;
		letter-spacing: -0.01em;
	}

	p {
		margin: var(--space-1) 0 0;
	}

	@media (max-width: 767px) {
		h1 {
			font-size: 24px;
		}
	}
</style>
