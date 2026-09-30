<script lang="ts">
	import AccountLink from './AccountLink.svelte';
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
	<div class="actions">
		{#if toggle}<ViewToggle />{/if}
		<!-- On a phone there is no sidebar, so the account is reached from here. -->
		<span class="account"><AccountLink /></span>
	</div>
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

	.actions {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.account {
		display: none;
	}

	@media (max-width: 767px) {
		.account {
			display: contents;
		}

		h1 {
			font-size: 24px;
		}
	}
</style>
