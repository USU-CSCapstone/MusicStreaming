<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { AdminList } from '$lib/admin-list.svelte';
	import {
		ApiError,
		adminChangeLibraryAccess,
		adminDeleteUser,
		adminIssuePasswordReset,
		adminListUsers,
		adminSetUserStatus
	} from '$lib/api/client';
	import type { AdminUser } from '$lib/api/types';
	import ConfirmDialog from '$lib/components/ConfirmDialog.svelte';
	import CopyLink from '$lib/components/CopyLink.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import AccountDevices from './AccountDevices.svelte';
	import AccountLibraries from './AccountLibraries.svelte';

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
	let resetting = $state<AdminUser | null>(null);
	let deleting = $state<AdminUser | null>(null);
	/** The reset link just made, held only here: the server does not give the code again. */
	let issued = $state<{ username: string; link: string } | null>(null);
	let listError = $state<string | null>(null);
	/** The accounts ticked, by ID, kept here so a reloaded list neither loses nor misapplies them. */
	let selected = $state<string[]>([]);
	let bulkLibrary = $state('');
	let bulkBusy = $state(false);
	let bulkError = $state<string | null>(null);
	/** The panel open on each account: its libraries or its devices. */
	let panel = $state<Record<string, 'libraries' | 'devices' | undefined>>({});
	const MOST_AT_ONCE = 1000;
	const blocked = $derived(!bulkLibrary || bulkBusy || selected.length > MOST_AT_ONCE);

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

	/** Makes the account's reset link, keeps it for showing, and marks the row as waiting for it. */
	async function issueReset(row: AdminUser, password: string) {
		const made = await adminIssuePasswordReset(fetch, row.id, { password });
		issued = {
			username: row.username,
			link: `${location.origin}${resolve('/reset')}#${made.code}`
		};
		list.replace({ ...row, resetPending: true }, (r) => r.id === row.id);
	}

	/**
	 * Deletes the account, then drops its row, its place in the selection and a reset link shown
	 * for it, which the deletion cancelled.
	 */
	async function remove(row: AdminUser, password: string) {
		await adminDeleteUser(fetch, row.id, { password });
		list.remove((r) => r.id === row.id);
		selected = selected.filter((id) => id !== row.id);
		if (issued?.username === row.username) issued = null;
	}

	function toggle(row: AdminUser, which: 'libraries' | 'devices') {
		panel[row.id] = panel[row.id] === which ? undefined : which;
	}

	/** Grants or revokes the chosen library for the selection, then reads the list again. */
	async function change(action: 'grant' | 'revoke') {
		bulkBusy = true;
		bulkError = null;
		try {
			await adminChangeLibraryAccess(fetch, {
				userIds: [...selected],
				libraryIds: [bulkLibrary],
				action
			});
			selected = [];
			await list.reload();
		} catch (err) {
			bulkError = message(err);
		} finally {
			bulkBusy = false;
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

<PageHeader
	title="Accounts"
	subtitle="Find an account, change what it can reach, and suspend or restore it."
/>

<section aria-labelledby="all-title">
	<h2 id="all-title">All accounts</h2>
	{#if issued}
		<div class="made">
			<h3>Reset link for {issued.username}</h3>
			<CopyLink url={issued.link} />
			<p class="muted small">
				It is shown only now, and lasts 24 hours. It works only where this address works: if you are
				using an address on your home network, people elsewhere cannot open it. Making another link
				replaces this one.
			</p>
			<button type="button" onclick={() => (issued = null)}>Done</button>
		</div>
	{/if}
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
		{#if selected.length > 0}
			<div class="bulk" role="group" aria-label="Selected accounts">
				<strong>{selected.length} selected</strong>
				<select aria-label="Library for the selection" bind:value={bulkLibrary} disabled={bulkBusy}>
					<option value="">Choose a library</option>
					{#each data.libraries as library (library.id)}
						<option value={library.id}>{library.name}</option>
					{/each}
				</select>
				<button type="button" disabled={blocked} onclick={() => change('grant')}>Grant</button>
				<button type="button" disabled={blocked} onclick={() => change('revoke')}>Revoke</button>
				<button type="button" disabled={bulkBusy} onclick={() => (selected = [])}>Clear</button>
				{#if selected.length > MOST_AT_ONCE}
					<p class="muted small">Change at most 1,000 accounts at once. Clear some.</p>
				{/if}
				{#if bulkError}<p class="error" role="alert">{bulkError}</p>{/if}
			</div>
		{/if}
		<ul class="accounts">
			{#each list.items as row (row.id)}
				<li class="card">
					<div class="top">
						{#if outranks(row)}
							<input
								type="checkbox"
								aria-label="Select {row.username}"
								value={row.id}
								bind:group={selected}
							/>
						{/if}
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
					{#if outranks(row)}
						<div class="panels">
							<button
								type="button"
								aria-expanded={panel[row.id] === 'libraries'}
								onclick={() => toggle(row, 'libraries')}
							>
								Libraries
							</button>
							<button
								type="button"
								aria-expanded={panel[row.id] === 'devices'}
								onclick={() => toggle(row, 'devices')}
							>
								Devices
							</button>
							<button type="button" onclick={() => (resetting = row)}>Reset password</button>
							<button type="button" onclick={() => (deleting = row)}>Delete</button>
						</div>
						{#if panel[row.id] === 'libraries'}
							<!-- Restarts on a new row, so the boxes show the grants the server now has. -->
							{#key row}
								<AccountLibraries
									{row}
									libraries={data.libraries}
									onsaved={(user) => list.replace(user, (r) => r.id === row.id)}
								/>
							{/key}
						{:else if panel[row.id] === 'devices'}
							<AccountDevices {row} />
						{/if}
					{/if}
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

{#if resetting}
	{@const row = resetting}
	<ConfirmDialog
		title={`Reset ${row.username}’s password?`}
		confirmLabel="Reset password"
		password
		action={(password) => issueReset(row, password)}
		onclose={() => (resetting = null)}
	>
		<p>
			You get a link to give {row.username}, who uses it to choose a new password. Using it signs
			them out of every device.
		</p>
	</ConfirmDialog>
{/if}

{#if deleting}
	{@const row = deleting}
	<ConfirmDialog
		title={`Delete ${row.username}?`}
		confirmLabel="Delete"
		danger
		password
		action={(password) => remove(row, password)}
		onclose={() => (deleting = null)}
	>
		<p>
			{row.username}’s playlists, listening history, likes and devices go for good, and they are
			signed out everywhere. No music files are touched. Plugins {row.username} connected keep what they
			already received. This cannot be undone.
		</p>
	</ConfirmDialog>
{/if}

<style>
	section {
		max-width: 760px;
		margin-bottom: var(--space-6);
	}

	.made {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-2);
		margin-bottom: var(--space-4);
	}

	.made h3 {
		margin: 0;
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

	.top input[type='checkbox'] {
		flex-shrink: 0;
		width: 20px;
		height: 20px;
	}

	.who {
		flex: 1;
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.panels,
	.bulk {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-3);
	}

	.bulk {
		margin-bottom: var(--space-4);
	}

	.bulk select {
		min-width: 0;
		height: 36px;
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font: inherit;
	}

	.panels button[aria-expanded='true'] {
		background: var(--accent-soft);
		color: var(--accent);
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
