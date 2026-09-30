<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { ApiError, completeSetup } from '$lib/api/client';
	import { thisDevice } from '$lib/device';

	let username = $state('');
	let displayName = $state('');
	let password = $state('');
	let confirm = $state('');
	let error = $state('');
	let busy = $state(false);

	async function onsubmit(e: SubmitEvent) {
		e.preventDefault();
		// No one can reset the owner's password for them, so a typo here would lock them out.
		if (password !== confirm) {
			error = "The passwords don't match.";
			return;
		}
		busy = true;
		error = '';
		try {
			await completeSetup(fetch, {
				username,
				password,
				displayName: displayName.trim() || undefined,
				device: thisDevice()
			});
			await goto(resolve('/'), { invalidateAll: true });
		} catch (e) {
			// Someone else finished setup first; the app shows what they set up.
			if (e instanceof ApiError && e.status === 404) {
				await goto(resolve('/'), { invalidateAll: true });
				return;
			}
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>Set up Jewelcase</title>
</svelte:head>

<main>
	<form {onsubmit}>
		<h1>Set up Jewelcase</h1>
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

		{#if error}
			<p class="error" role="alert">{error}</p>
		{/if}

		<button type="submit" disabled={busy}>{busy ? 'Creating account…' : 'Create account'}</button>
	</form>
</main>

<style>
	main {
		display: grid;
		place-items: center;
		min-height: 100dvh;
		padding: var(--space-4);
	}

	form {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		width: 100%;
		max-width: 380px;
		padding: var(--space-6);
		border-radius: var(--radius-l);
		background: var(--surface);
		box-shadow: var(--shadow);
	}

	h1 {
		font-size: 22px;
	}

	p {
		margin: 0;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		font-weight: 500;
	}

	label .muted {
		font-weight: 400;
	}

	input {
		height: 36px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface-2);
		color: var(--text);
		font: inherit;
		font-weight: 400;
	}

	input:focus-visible {
		outline-offset: -1px;
	}

	.error {
		color: var(--danger);
	}

	button {
		height: 40px;
		border: 0;
		border-radius: var(--radius-m);
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		cursor: pointer;
	}

	button:disabled {
		opacity: 0.6;
		cursor: default;
	}
</style>
