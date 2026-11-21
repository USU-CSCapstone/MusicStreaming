<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { AdminList } from '$lib/admin-list.svelte';
	import { ApiError, adminListUsers, adminSetUserStatus } from '$lib/api/client';
	import type { AdminUser } from '$lib/api/types';
	import ConfirmDialog from '$lib/components/ConfirmDialog.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';

	let { data } = $props();

	// Reset when the search changes or the page reloads its data.
	const list = $derived(
		new AdminList(data.users, (cursor) => adminListUsers(fetch, { q: data.q, cursor }))
	);

	const RANK = { user: 0, admin: 1, owner: 2 };
	const ROLES = { owner: 'Owner', admin: 'Admin', user: 'User' };

	/** Whether the viewer may act on `row`: only on an account they outrank, as the server decides. */
	const outranks = (row: AdminUser) => !!data.me && RANK[data.me.role] > RANK[row.role];

	let query = $derived(data.q);
	let suspending = $state<AdminUser | null>(null);
	let listError = $state<string | null>(null);
	let busy = $state<Record<string, boolean>>({});
	let rowError = $state<Record<string, string>>({});

	function message(e: unknown): string {
		return e instanceof ApiError ? e.message : 'Something went wrong. Try again.';
	}

	function search(e: SubmitEvent) {
		e.preventDefault();
		const params = new URLSearchParams({ q: query.trim() });
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolve() plus a query string
		void goto(`${resolve('/admin/accounts')}?${params}`, { keepFocus: true });
	}

	/** Sets the account's status and shows it as the server answers. */
	async function setStatus(row: AdminUser, status: AdminUser['status']) {
		list.replace(await adminSetUserStatus(fetch, row.id, status), (r) => r.id === row.id);
	}

	async function restore(row: AdminUser) {
		busy[row.id] = true;
		delete rowError[row.id];
		try {
			await setStatus(row, 'active');
		} catch (err) {
			rowError[row.id] = message(err);
		} finally {
			busy[row.id] = false;
		}
	}

	async function more() {
		listError = null;
		try {
			await list.loadMore();
		} catch (err) {
			listError = message(err);
		}
	}
</script>

<PageHeader title="Accounts" subtitle="Find an account, and suspend or restore it." />

<section aria-labelledby="all-title">
	<h2 id="all-title">All accounts</h2>
	<form class="search" role="search" onsubmit={search}>
		<input
			type="search"
			aria-label="Search accounts"
			placeholder="Name or username"
			bind:value={query}
		/>
		<button type="submit">Search</button>
	</form>
	{#if list.items.length === 0}
		<p class="muted">{data.q ? 'No account matches that search.' : 'No accounts.'}</p>
	{:else}
		<ul class="accounts">
			{#each list.items as row (row.id)}
				<li class="card">
					<div class="top">
						<div class="who">
							<strong>{row.displayName}</strong>
							<span class="muted small">{row.username}</span>
						</div>
						{#if outranks(row)}
							{#if row.status === 'suspended'}
								<button type="button" disabled={busy[row.id]} onclick={() => restore(row)}>
									Restore
								</button>
							{:else}
								<button type="button" onclick={() => (suspending = row)}>Suspend</button>
							{/if}
						{/if}
					</div>
					<p class="tags small">
						<span>{ROLES[row.role]}</span>
						{#if row.status === 'suspended'}<span class="flag">Suspended</span>{/if}
						{#if row.resetPending}<span class="flag">Reset pending</span>{/if}
					</p>
					{#if rowError[row.id]}<p class="error" role="alert">{rowError[row.id]}</p>{/if}
				</li>
			{/each}
		</ul>
		{#if listError}<p class="error" role="alert">{listError}</p>{/if}
		{#if list.hasMore}
			<button type="button" class="more" onclick={more}>Load more</button>
		{/if}
	{/if}
</section>

{#if suspending}
	{@const row = suspending}
	<ConfirmDialog
		title={`Suspend ${row.username}?`}
		confirmLabel="Suspend"
		danger
		action={() => setStatus(row, 'suspended')}
		onclose={() => (suspending = null)}
	>
		<p>
			{row.username} is signed out of every device at once and cannot sign in until you restore the account.
			Nothing they have is lost.
		</p>
	</ConfirmDialog>
{/if}

<style>
	section {
		max-width: 760px;
		margin-bottom: var(--space-6);
	}

	.search {
		display: flex;
		gap: var(--space-3);
		margin-bottom: var(--space-4);
	}

	.search input {
		flex: 1;
		min-width: 0;
		height: 36px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font: inherit;
	}

	.accounts {
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

	.top {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
	}

	.who {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.tags {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-3);
	}

	.flag {
		color: var(--accent);
		font-weight: 600;
	}

	p {
		margin: 0;
	}

	.small {
		font-size: 13px;
	}

	button {
		flex-shrink: 0;
		min-height: 36px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 18px;
		background: var(--surface-2);
		color: inherit;
		font-weight: 600;
		cursor: pointer;
	}

	.search button {
		background: var(--accent);
		color: var(--accent-text);
	}

	button:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.error {
		color: var(--danger);
	}
</style>
