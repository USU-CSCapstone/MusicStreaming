<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';

	let { showName = false }: { showName?: boolean } = $props();

	const me = $derived(page.data.me);
	// Avatars are not served yet (`hasAvatar`), so every account shows its initial.
	const initial = $derived(me?.displayName.trim().charAt(0).toUpperCase() ?? '');
</script>

{#if me}
	<a
		href={resolve('/account')}
		aria-label={showName ? undefined : `Account: ${me.displayName}`}
		aria-current={page.route.id === '/account' ? 'page' : undefined}
	>
		<span class="avatar" aria-hidden="true">{initial}</span>
		{#if showName}<span class="name">{me.displayName}</span>{/if}
	</a>
{/if}

<style>
	a {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-width: 0;
		border-radius: var(--radius-m);
		color: var(--text);
		font-weight: 500;
	}

	a:hover {
		text-decoration: none;
	}

	.avatar {
		display: grid;
		place-items: center;
		flex: none;
		width: 32px;
		height: 32px;
		border-radius: 50%;
		background: var(--accent-soft);
		color: var(--accent);
		font-weight: 600;
	}

	.name {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
