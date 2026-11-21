<script lang="ts">
	import { base } from '$app/paths';
	import { AdminList } from '$lib/admin-list.svelte';
	import {
		ApiError,
		adminCreateInvite,
		adminListInvites,
		adminRevokeInvite
	} from '$lib/api/client';
	import type { Invite } from '$lib/api/types';
	import CopyLink from '$lib/components/CopyLink.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';

	let { data } = $props();

	// Reset when the page reloads its data.
	const list = $derived(new AdminList(data.invites, (cursor) => adminListInvites(fetch, cursor)));
	const libraryNames = $derived(new Map(data.libraries.map((l) => [l.id, l.name])));

	const STATES: Record<Invite['status'], string> = {
		pending: 'Pending',
		redeemed: 'Redeemed',
		revoked: 'Revoked',
		expired: 'Expired'
	};

	let chosen = $state<string[]>([]);
	let days = $state(7);
	let creating = $state(false);
	let formError = $state<string | null>(null);
	/** The link of the invite just made, held only here: the server does not give the code again. */
	let link = $state<string | null>(null);
	let listError = $state<string | null>(null);
	let busy = $state<Record<string, boolean>>({});
	let rowError = $state<Record<string, string>>({});

	function message(e: unknown): string {
		return e instanceof ApiError ? e.message : 'Something went wrong. Try again.';
	}

	async function create(e: SubmitEvent) {
		e.preventDefault();
		creating = true;
		formError = null;
		try {
			const made = await adminCreateInvite(fetch, { libraryIds: chosen, expiresInDays: days });
			link = `${location.origin}${base}/invite#${made.code}`;
			chosen = [];
			await list.reload().catch(() => {});
		} catch (err) {
			formError = message(err);
		} finally {
			creating = false;
		}
	}

	async function revoke(invite: Invite) {
		busy[invite.id] = true;
		delete rowError[invite.id];
		try {
			list.replace(await adminRevokeInvite(fetch, invite.id), (row) => row.id === invite.id);
		} catch (err) {
			rowError[invite.id] = message(err);
		} finally {
			busy[invite.id] = false;
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

	const when = (iso: string) => new Date(iso).toLocaleString();

	/** The dates that matter for the invite's state. */
	function dates(i: Invite): string {
		const made = `Created ${when(i.createdAt)}`;
		if (i.status === 'redeemed' && i.redeemedAt) return `${made} · Redeemed ${when(i.redeemedAt)}`;
		if (i.status === 'pending') return `${made} · Expires ${when(i.expiresAt)}`;
		return i.status === 'expired' ? `${made} · Expired ${when(i.expiresAt)}` : made;
	}
</script>

<PageHeader title="Invites" subtitle="Let people join with a link you share." />

<section aria-labelledby="new-title">
	<h2 id="new-title">New invite</h2>
	<form onsubmit={create}>
		<fieldset disabled={creating}>
			<legend>Libraries they can reach</legend>
			{#if data.libraries.length === 0}
				<p class="muted">No libraries yet.</p>
			{/if}
			{#each data.libraries as library (library.id)}
				<label class="check">
					<input type="checkbox" value={library.id} bind:group={chosen} />
					{library.name}
				</label>
			{/each}
		</fieldset>
		<label class="days">
			Days until it expires
			<input
				type="number"
				min="1"
				max="30"
				step="1"
				required
				bind:value={days}
				disabled={creating}
			/>
		</label>
		<button type="submit" disabled={creating}>{creating ? 'Creating…' : 'Create invite'}</button>
	</form>
	{#if formError}<p class="error" role="alert">{formError}</p>{/if}
	{#if link}
		<div class="made">
			<h3>Share this link</h3>
			<CopyLink url={link} />
			<p class="muted small">
				It is shown only now, and works once. It works only where this address works: if you are
				using an address on your home network, people elsewhere cannot open it.
			</p>
		</div>
	{/if}
</section>

<section aria-labelledby="all-title">
	<h2 id="all-title">All invites</h2>
	{#if list.items.length === 0}
		<p class="muted">No invites yet.</p>
	{:else}
		<ul class="invites">
			{#each list.items as invite (invite.id)}
				<li class="card">
					<div class="top">
						<strong class="state {invite.status}">{STATES[invite.status]}</strong>
						{#if invite.status === 'pending'}
							<button type="button" disabled={busy[invite.id]} onclick={() => revoke(invite)}>
								Revoke
							</button>
						{/if}
					</div>
					<p class="small">
						Made by {invite.createdBy?.username ?? 'a removed account'}{#if invite.redeemedBy}, used
							by {invite.redeemedBy.username}{/if}
					</p>
					<p class="muted small">{dates(invite)}</p>
					<p class="small">
						{#if invite.libraryIds.length === 0}
							No libraries
						{:else}
							Libraries: {invite.libraryIds
								.map((id) => libraryNames.get(id) ?? 'a removed library')
								.join(', ')}
						{/if}
					</p>
					{#if rowError[invite.id]}<p class="error" role="alert">{rowError[invite.id]}</p>{/if}
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
		min-width: 0;
		margin: 0;
		padding: 0;
		border: 0;
	}

	legend {
		margin-bottom: var(--space-2);
		font-weight: 600;
	}

	.check,
	.days {
		display: flex;
		gap: var(--space-2);
	}

	.days {
		flex-direction: column;
	}

	input[type='number'] {
		width: 100px;
		height: 36px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font: inherit;
	}

	.made {
		margin-top: var(--space-4);
	}

	.invites {
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

	.pending {
		color: var(--accent);
	}

	p {
		margin: 0;
	}

	.small {
		font-size: 13px;
	}

	button {
		height: 34px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 17px;
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		cursor: pointer;
	}

	.top button,
	.more {
		background: var(--surface-2);
		color: inherit;
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
