<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { ApiError, completeSetup } from '$lib/api/client';
	import AuthForm from '$lib/components/AuthForm.svelte';
	import { thisDevice } from '$lib/device';

	let username = $state('');
	let displayName = $state('');
	let password = $state('');
	let confirm = $state('');

	async function setUp() {
		// No one can reset the owner's password for them, so a typo here would lock them out.
		if (password !== confirm) throw new Error("The passwords don't match.");
		try {
			await completeSetup(fetch, {
				username,
				password,
				displayName: displayName.trim() || undefined,
				device: thisDevice()
			});
		} catch (e) {
			// Someone else finished setup first; the app shows what they set up.
			if (!(e instanceof ApiError && e.status === 404)) throw e;
		}
		await goto(resolve('/'), { invalidateAll: true });
	}
</script>

<svelte:head>
	<title>Set up Jewelcase</title>
</svelte:head>

<AuthForm
	title="Set up Jewelcase"
	submit="Create account"
	busyLabel="Creating account…"
	action={setUp}
>
	<p class="muted">
		Create the owner account. The owner runs this server: its libraries, its settings, and who can
		use it.
	</p>

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
	<label>
		Confirm password
		<input type="password" bind:value={confirm} required autocomplete="new-password" />
	</label>
</AuthForm>
