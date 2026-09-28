<script lang="ts">
	import { page } from '$app/state';
	import { NO_LIBRARY } from '$lib/library';
</script>

<div class="state">
	{#if page.error?.message === NO_LIBRARY}
		<h1>No music yet</h1>
		<p class="muted">
			There is no library to browse. Once the server has scanned a music folder, it appears here.
		</p>
	{:else if page.status === 404}
		<h1>Not found</h1>
		<p class="muted">This page doesn't exist, or isn't in your library.</p>
	{:else}
		<h1>Something went wrong</h1>
		<p class="muted">{page.error?.message}</p>
	{/if}
</div>

<style>
	.state {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		max-width: 420px;
		margin: 12vh auto 0;
		text-align: center;
	}

	h1 {
		font-size: 22px;
	}
</style>
