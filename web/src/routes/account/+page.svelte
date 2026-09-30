<script lang="ts">
	import { isRedirect } from '@sveltejs/kit';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { logout } from '$lib/api/client';
	import type { User } from '$lib/api/types';
	import PageHeader from '$lib/components/PageHeader.svelte';

	const me = $derived(page.data.me);
	const roles: Record<User['role'], string> = { owner: 'Owner', admin: 'Admin', user: 'User' };

	let error = $state('');
	let busy = $state(false);

	async function logOut() {
		busy = true;
		error = '';
		try {
			await logout(fetch);
		} catch (e) {
			// Already logged out elsewhere is as good as logging out now.
			if (!isRedirect(e)) {
				error = e instanceof Error ? e.message : String(e);
				busy = false;
				return;
			}
		}
		// A fresh start, so nothing of this session, playback above all, outlives it.
		location.assign(resolve('/login'));
	}
</script>

<svelte:head><title>Account · Jewelcase</title></svelte:head>

<PageHeader title="Account" />
{#if me}
	<section>
		<p>
			<strong>{me.displayName}</strong>
			<span class="muted">{me.username} · {roles[me.role]}</span>
		</p>
		<!-- Told in the app, not only in documentation (`requirements/users.md` §8). -->
		<p class="muted">
			The admins of this server can see your listening data: your playlists, queues, downloads,
			history, and statistics. They can't sign in as you or change any of it.
		</p>
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<button type="button" onclick={logOut} disabled={busy}>
			{busy ? 'Logging out…' : 'Log out'}
		</button>
	</section>
{/if}

<style>
	section {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-3);
		max-width: 560px;
	}

	p {
		margin: 0;
	}

	strong {
		display: block;
		font-size: 16px;
	}

	.error {
		color: var(--danger);
	}

	button {
		height: 36px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		font-weight: 500;
		cursor: pointer;
	}

	button:hover {
		background: var(--surface-hover);
	}

	button:disabled {
		opacity: 0.6;
		cursor: default;
	}
</style>
