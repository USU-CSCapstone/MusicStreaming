<script lang="ts">
	import { AdminList } from '$lib/admin-list.svelte';
	import { ApiError, adminListAuditLog } from '$lib/api/client';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { entryText } from './entry-text';

	let { data } = $props();

	// Reset when the page reloads its data.
	const list = $derived(
		new AdminList(data.audit, (cursor) => adminListAuditLog(fetch, { cursor }))
	);
	const libraryNames = $derived(new Map(data.libraries.map((l) => [l.id, l.name])));

	let listError = $state<string | null>(null);

	async function more() {
		listError = null;
		try {
			await list.loadMore();
		} catch (err) {
			listError = err instanceof ApiError ? err.message : 'Something went wrong. Try again.';
		}
	}
</script>

<PageHeader title="Audit log" subtitle="Who did what to which account, newest first." />

<section aria-label="Entries">
	{#if list.items.length === 0}
		<p class="muted">Nothing has been recorded yet.</p>
	{:else}
		<ul class="entries">
			{#each list.items as entry (entry.id)}
				<li class="card">
					<p>{entryText(entry, libraryNames)}</p>
					<time class="muted small" datetime={entry.at}>{new Date(entry.at).toLocaleString()}</time>
				</li>
			{/each}
		</ul>
		{#if listError}<p class="error" role="alert">{listError}</p>{/if}
		{#if list.hasMore}
			<button type="button" class="more" onclick={more}>Load more</button>
		{/if}
	{/if}
</section>

<style>
	section {
		max-width: 760px;
		margin-bottom: var(--space-6);
	}

	.entries {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		margin: 0 0 var(--space-3);
		padding: 0;
		list-style: none;
	}

	.card {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		padding: var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-l);
		background: var(--surface);
		overflow-wrap: anywhere;
	}

	p {
		margin: 0;
	}

	.small {
		font-size: 13px;
	}

	.more {
		height: 34px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 17px;
		background: var(--surface-2);
		color: inherit;
		font-weight: 600;
		cursor: pointer;
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
