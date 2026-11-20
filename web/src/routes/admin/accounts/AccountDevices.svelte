<script lang="ts">
	import {
		ApiError,
		adminListUserDevices,
		adminSignOutDevice,
		adminSignOutDevices
	} from '$lib/api/client';
	import type { AdminUser, Device } from '$lib/api/types';
	import ConfirmDialog from '$lib/components/ConfirmDialog.svelte';

	let { row }: { row: AdminUser } = $props();

	// Read when opened, not in the page's load: a refusal for rank is this panel's to show.
	let devices = $state<Device[] | null>(null);
	let error = $state<string | null>(null);
	let busy = $state<string | null>(null);
	let confirming = $state(false);

	function message(e: unknown): string {
		return e instanceof ApiError ? e.message : 'Something went wrong. Try again.';
	}

	async function read() {
		try {
			devices = (await adminListUserDevices(fetch, row.id)).items;
		} catch (err) {
			error = message(err);
		}
	}
	void read();

	async function signOut(device: Device) {
		busy = device.id;
		error = null;
		try {
			await adminSignOutDevice(fetch, row.id, device.id);
			await read();
		} catch (err) {
			error = message(err);
		} finally {
			busy = null;
		}
	}

	async function signOutAll() {
		await adminSignOutDevices(fetch, row.id);
		await read();
	}

	const seen = (device: Device) => new Date(device.lastSeenAt).toLocaleString();
</script>

{#if devices === null}
	{#if !error}<p class="muted">Loading…</p>{/if}
{:else if devices.length === 0}
	<p class="muted">No signed-in devices.</p>
{:else}
	<ul aria-label="{row.username}’s devices">
		{#each devices as device (device.id)}
			<li>
				<span class="meta">
					<span class="name">{device.name}</span>
					<span class="muted small">
						{device.connected ? 'Connected' : 'Not connected'} · last seen {seen(device)}
					</span>
				</span>
				<button
					type="button"
					aria-label="Sign out {device.name}"
					disabled={busy !== null}
					onclick={() => signOut(device)}
				>
					Sign out
				</button>
			</li>
		{/each}
	</ul>
	<button type="button" class="all" onclick={() => (confirming = true)}>Sign out all</button>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if confirming}
	<ConfirmDialog
		title={`Sign out ${row.username} everywhere?`}
		confirmLabel="Sign out all"
		danger
		action={signOutAll}
		onclose={() => (confirming = false)}
	>
		<p>{row.username} is signed out of every device at once, and can sign in again.</p>
	</ConfirmDialog>
{/if}

<style>
	ul {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		margin: 0 0 var(--space-3);
		padding: 0;
		list-style: none;
	}

	li {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
	}

	.meta {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.name {
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
