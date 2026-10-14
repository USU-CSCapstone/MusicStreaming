<script lang="ts">
	import { toaster } from '$lib/toast.svelte';
	import Icon from './Icon.svelte';

	// Focus moving between the toast's own buttons neither arrives nor leaves.
	const fromOutside = (e: FocusEvent) =>
		!(e.currentTarget as HTMLElement).contains(e.relatedTarget as Node | null);

	function onfocusin(e: FocusEvent) {
		if (fromOutside(e)) toaster.hold();
	}

	function onfocusout(e: FocusEvent) {
		if (fromOutside(e)) toaster.release();
	}
</script>

<!-- The live region stays mounted, so screen readers announce each message as it appears. -->
<div class="region" role="status" aria-live="polite">
	{#if toaster.current}
		{@const toast = toaster.current}
		<!-- Pointing at the toast only stops its clock, as focus inside it does for the keyboard. -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="toast"
			onpointerenter={() => toaster.hold()}
			onpointerleave={() => toaster.release()}
			{onfocusin}
			{onfocusout}
		>
			<span class="message">{toast.message}</span>
			{#if toast.action}
				<button type="button" class="action" onclick={() => toaster.act()}>
					{toast.action.label}
				</button>
			{/if}
			<button type="button" class="dismiss" aria-label="Dismiss" onclick={() => toaster.dismiss()}>
				<Icon name="close" size={16} />
			</button>
		</div>
	{/if}
</div>

<style>
	.region {
		position: absolute;
		inset: auto 0 var(--space-4);
		z-index: 20;
		display: grid;
		justify-items: center;
		padding: 0 var(--space-4);
		/* Only the toast itself takes the pointer, not the strip it sits in. */
		pointer-events: none;
	}

	.toast {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		max-width: 480px;
		min-height: 48px;
		padding: var(--space-2) var(--space-2) var(--space-2) var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		box-shadow: var(--shadow);
		pointer-events: auto;
		animation: rise 0.18s ease-out;
	}

	.message {
		flex: 1;
		min-width: 0;
	}

	button {
		flex-shrink: 0;
		border: 0;
		background: transparent;
		cursor: pointer;
	}

	.action {
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-s);
		color: var(--accent);
		font-weight: 600;
	}

	.action:hover {
		background: var(--accent-soft);
	}

	.dismiss {
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		border-radius: 50%;
		color: var(--text-muted);
	}

	.dismiss:hover {
		background: var(--surface-hover);
		color: var(--text);
	}

	@keyframes rise {
		from {
			opacity: 0;
			transform: translateY(8px);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.toast {
			animation: none;
		}
	}
</style>
