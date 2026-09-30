<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { ApiError, login } from '$lib/api/client';
	import AuthForm from '$lib/components/AuthForm.svelte';
	import { thisDevice } from '$lib/device';

	let username = $state('');
	let password = $state('');

	/** Where to go once logged in: the `next` page, if it is one of this app's. */
	function next(): string {
		const target = new URL(page.url.searchParams.get('next') ?? '', page.url);
		return target.origin === page.url.origin && target.pathname !== page.url.pathname
			? target.pathname + target.search
			: resolve('/');
	}

	async function logIn() {
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
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- a path of this app, checked in next()
		await goto(next(), { invalidateAll: true });
	}
</script>

<svelte:head>
	<title>Log in · Jewelcase</title>
</svelte:head>

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
