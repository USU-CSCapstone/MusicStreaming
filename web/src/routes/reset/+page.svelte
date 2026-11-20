<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { API, ApiError, checkPasswordReset, redeemPasswordReset } from '$lib/api/client';
	import type { User } from '$lib/api/types';
	import AuthForm from '$lib/components/AuthForm.svelte';
	import { connectivity } from '$lib/connectivity.svelte';

	const INVALID = 'This link is no longer valid.';

	// The code is in the fragment, which no request carries; it leaves the address bar at once, so
	// it is not kept in the browser's history or seen on screen. SvelteKit's state stays as it was.
	const code = location.hash.slice(1);
	history.replaceState(history.state, '', location.pathname + location.search);

	let password = $state('');
	let again = $state('');

	/**
	 * What shows instead of the form, or the account's username where the form shows. Someone signed
	 * in is asked to sign out first and nothing else is done, so their session and device are left
	 * as they are. `/me` is fetched as it is, as the client's `getMe` would wipe the device on a
	 * `401`.
	 */
	async function look(): Promise<{ message: string } | { username: string }> {
		const response = await fetch(`${API}/me`);
		if (response.status !== 401) {
			const me: User | null = response.ok ? await response.json() : null;
			return {
				message: `You're signed in${me ? ` as ${me.displayName}` : ''}. Sign out first to use this link.`
			};
		}
		// The code left the address bar as the page opened, so a reload, or a tab the browser
		// reopened, has none; the link the person was sent still has it.
		if (!code) return { message: 'Open the reset link you were sent again to continue.' };
		// Opening the link is the choice to reach the server, and this page has no switch for it.
		if (connectivity.workOffline) connectivity.setWorkOffline(false);
		try {
			return await checkPasswordReset(fetch, code);
		} catch (e) {
			if (e instanceof ApiError && e.status === 404) return { message: INVALID };
			throw e;
		}
	}
	const found = look();

	/** Sets the new password and sends the person to log in with it; this signs no one in. */
	async function change() {
		if (password !== again) throw new Error('The passwords do not match.');
		try {
			await redeemPasswordReset(fetch, { code, password });
		} catch (e) {
			if (e instanceof ApiError && e.status === 404) throw new Error(INVALID, { cause: e });
			throw e;
		}
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolve() plus a query string
		await goto(`${resolve('/login')}?reset=1`);
	}
</script>

<svelte:head>
	<title>Reset password · Jewelcase</title>
</svelte:head>

{#await found}
	<p class="message" role="status">Checking the link…</p>
{:then found}
	{#if 'message' in found}
		<p class="message">{found.message}</p>
	{:else}
		<AuthForm
			title="Choose a new password"
			submit="Change password"
			busyLabel="Changing password…"
			action={change}
		>
			<p class="muted">For the account <strong>{found.username}</strong>.</p>
			<label>
				New password
				<input type="password" bind:value={password} required autocomplete="new-password" />
			</label>
			<label>
				New password again
				<input type="password" bind:value={again} required autocomplete="new-password" />
			</label>
		</AuthForm>
	{/if}
{:catch error}
	<p class="message" role="alert">
		{error instanceof ApiError
			? error.message
			: 'The server can’t be reached. Try again in a moment.'}
	</p>
{/await}

<style>
	.message {
		max-width: 380px;
		margin: 20vh auto 0;
		padding: 0 var(--space-4);
		text-align: center;
	}
</style>
