<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { ApiError, login, sessionEnding } from '$lib/api/client';
	import AuthForm from '$lib/components/AuthForm.svelte';
	import { thisDevice } from '$lib/device';
	import { setLogoutPending } from '$lib/last-known';
	import { finishLogout } from '$lib/logout';

	let username = $state('');
	let password = $state('');
	/**
	 * Whether the server ended the session and the page is about to reload; no form shows meanwhile.
	 */
	const ending = sessionEnding();
	/** Where to go once logged in: the `next` page, if it is one of this app's. */
	function next(): string {
		const target = new URL(page.url.searchParams.get('next') ?? '', page.url);
		return target.origin === page.url.origin && target.pathname !== page.url.pathname
			? target.pathname + target.search
			: resolve('/');
	}

	async function logIn() {
		// A logout the server has not heard of is told first, or it would end the session this
		// login makes; one it still cannot take is dropped, as this login is the newer.
		await finishLogout(fetch);
		try {
			await login(fetch, { username, password, device: thisDevice() });
		} catch (e) {
			if (e instanceof ApiError && e.code === 'invalid_credentials') {
				throw new Error('The username or password is wrong.', { cause: e });
			}
			if (e instanceof ApiError && e.code === 'rate_limited') {
				const wait = e.retryAfter ? ` in ${e.retryAfter} s` : ' in a moment';
				throw new Error(`Too many attempts. Try again${wait}.`, { cause: e });
			}
			throw e;
		}
		setLogoutPending(false);
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- a path of this app, checked in next()
		await goto(next(), { invalidateAll: true });
	}
</script>

<svelte:head>
	<title>Log in · Jewelcase</title>
</svelte:head>

{#if ending}
	<!-- The device is being wiped, and the page reloads here once it is: nothing typed is lost. -->
	<p role="status">Signing out…</p>
{:else}
	<AuthForm title="Log in" submit="Log in" busyLabel="Logging in…" action={logIn}>
		<label>
			Username
			<input
				bind:value={username}
				required
				autocomplete="username"
				autocapitalize="off"
				spellcheck="false"
			/>
		</label>
		<label>
			Password
			<input type="password" bind:value={password} required autocomplete="current-password" />
		</label>
	</AuthForm>
{/if}
