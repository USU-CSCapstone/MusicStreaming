<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { API, ApiError, checkInvite, redeemInvite } from '$lib/api/client';
	import type { User } from '$lib/api/types';
	import AuthForm from '$lib/components/AuthForm.svelte';
	import { connectivity } from '$lib/connectivity.svelte';
	import { thisDevice } from '$lib/device';

	const INVALID = 'This link is no longer valid.';

	// The code is in the fragment, which no request carries; it leaves the address bar at once, so
	// it is not kept in the browser's history or seen on screen. SvelteKit's state stays as it was.
	const code = location.hash.slice(1);
	history.replaceState(history.state, '', location.pathname + location.search);

	let username = $state('');
	let displayName = $state('');
	let password = $state('');

	/**
	 * What shows instead of the form, or nothing where the form shows. Someone signed in is asked
	 * to sign out first and nothing else is done, so their session and device are left as they are.
	 * `/me` is fetched as it is, as the client's `getMe` would wipe the device on a `401`.
	 */
	async function look(): Promise<string | null> {
		const response = await fetch(`${API}/me`);
		if (response.status !== 401) {
			const me: User | null = response.ok ? await response.json() : null;
			return `You're signed in${me ? ` as ${me.displayName}` : ''}. Sign out first to use this invite.`;
		}
		// The code left the address bar as the page opened, so a reload, or a tab the browser
		// reopened, has none; the link the person was sent still has it.
		if (!code) return 'Open the invite link you were sent again to continue.';
		// Opening the link is the choice to reach the server, and this page has no switch for it.
		if (connectivity.workOffline) connectivity.setWorkOffline(false);
		try {
			await checkInvite(fetch, code);
			return null;
		} catch (e) {
			if (e instanceof ApiError && e.status === 404) return INVALID;
			throw e;
		}
	}
	const message = look();

	/** Creates the account, signed in on this device, and goes into the app as setup does. */
	async function join() {
		try {
			await redeemInvite(fetch, {
				code,
				username,
				password,
				displayName: displayName.trim() || undefined,
				device: thisDevice()
			});
		} catch (e) {
			if (e instanceof ApiError && e.status === 404) throw new Error(INVALID, { cause: e });
			throw e;
		}
		await goto(resolve('/'), { invalidateAll: true });
	}
</script>

<svelte:head>
	<title>Join · Jewelcase</title>
</svelte:head>

{#await message}
	<p class="message" role="status">Checking the link…</p>
{:then message}
	{#if message}
		<p class="message">{message}</p>
	{:else}
		<AuthForm
			title="Join Jewelcase"
			submit="Create account"
			busyLabel="Creating account…"
			action={join}
		>
			<p class="muted">Choose a username and password for your new account.</p>
			<label>
				Username
				<input
					bind:value={username}
					required
					maxlength="32"
					pattern="[A-Za-z0-9_\-]+"
					title="Letters, digits, _ and -"
					autocomplete="username"
					autocapitalize="off"
					spellcheck="false"
				/>
			</label>
			<label>
				<span>Display name <span class="muted">(optional)</span></span>
				<input bind:value={displayName} maxlength="100" autocomplete="name" />
			</label>
			<label>
				Password
				<input type="password" bind:value={password} required autocomplete="new-password" />
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
