<script lang="ts">
	// The sorts of a list too narrow for its column headings, one button away
	// (`requirements/conventions.md` §4.1). It looks and works as AddToPlaylistMenu does.
	import { flushSync, tick } from 'svelte';
	import type { SortField, SortOrder, TrackSort } from '$lib/sorting';
	import Icon from './Icon.svelte';

	let {
		fields,
		labels,
		sort,
		onchoose
	}: {
		fields: readonly SortField[];
		labels: Record<SortField, string>;
		sort: TrackSort;
		/** A field chosen, which steps its sort as a click on its heading does. */
		onchoose: (field: SortField) => void;
	} = $props();

	let open = $state(false);
	let button = $state<HTMLButtonElement>();
	let menu = $state<HTMLElement>();
	let place = $state({ top: 0, left: 0 });

	const said = (order: SortOrder) => (order === 'asc' ? 'ascending' : 'descending');

	async function show() {
		open = true;
		await tick();
		position();
		const all = items();
		(all.find((i) => i.getAttribute('aria-checked') === 'true') ?? all[0])?.focus();
	}

	/** Focus goes back to the button. */
	function close() {
		open = false;
		button?.focus();
	}

	function choose(field: SortField) {
		onchoose(field);
		close();
	}

	/** Under the button, lined up on the right, or above it when there is no room below. */
	function position() {
		if (!menu || !button) return;
		const a = button.getBoundingClientRect();
		const { width, height } = menu.getBoundingClientRect();
		const below = a.bottom + 4;
		const top = below + height > innerHeight - 8 ? Math.max(8, a.top - 4 - height) : below;
		const left = Math.min(Math.max(8, a.right - width), innerWidth - width - 8);
		place = { top, left };
	}

	const items = () => [...(menu?.querySelectorAll<HTMLElement>('[role="menuitemradio"]') ?? [])];

	/** Arrow keys move between items, Escape closes, and Tab closes and moves on, as in any menu. */
	function onkeydown(e: KeyboardEvent) {
		const all = items();
		const at = all.indexOf(document.activeElement as HTMLElement);
		const go = (i: number) => all[(i + all.length) % all.length]?.focus();
		if (e.key === 'Tab') {
			// Gone before the browser moves focus on from the button, so it moves past the menu
			// rather than back into it, and the press isn't spent on closing.
			flushSync(close);
			return;
		}
		if (e.key === 'ArrowDown') go(at + 1);
		else if (e.key === 'ArrowUp') go(at - 1);
		else if (e.key === 'Home') go(0);
		else if (e.key === 'End') go(all.length - 1);
		else if (e.key === 'Escape') close();
		else return;
		e.preventDefault();
		e.stopPropagation();
	}

	// It belongs beside its button, so it goes when the page scrolls or a press lands elsewhere.
	$effect(() => {
		if (!open) return;
		const away = (e: Event) => {
			const target = e.target as Node;
			if (!menu?.contains(target) && !button?.contains(target)) open = false;
		};
		addEventListener('pointerdown', away, true);
		addEventListener('scroll', away, true);
		return () => {
			removeEventListener('pointerdown', away, true);
			removeEventListener('scroll', away, true);
		};
	});
</script>

<button
	bind:this={button}
	type="button"
	class="trigger"
	aria-haspopup="menu"
	aria-expanded={open}
	aria-label={sort ? `Sort, by ${labels[sort.field].toLowerCase()} ${said(sort.order)}` : 'Sort'}
	onclick={() => (open ? close() : show())}
>
	{#if sort}
		{labels[sort.field]}
		<Icon name={sort.order === 'asc' ? 'up' : 'down'} size={14} />
	{:else}
		Sort
	{/if}
</button>

{#if open}
	<div
		class="menu"
		role="menu"
		tabindex="-1"
		aria-label="Sort by"
		bind:this={menu}
		style:top="{place.top}px"
		style:left="{place.left}px"
		{onkeydown}
	>
		<p class="heading muted" aria-hidden="true">Sort by</p>
		{#each fields as field (field)}
			{@const order = sort?.field === field ? sort.order : null}
			<button
				type="button"
				role="menuitemradio"
				aria-checked={order !== null}
				onclick={() => choose(field)}
			>
				<span class="label">{labels[field]}</span>
				{#if order}
					<span class="order muted">{order === 'asc' ? 'Ascending' : 'Descending'}</span>
					<Icon name={order === 'asc' ? 'up' : 'down'} size={14} />
				{/if}
			</button>
		{/each}
	</div>
{/if}

<style>
	.trigger {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		height: 32px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: 16px;
		background: var(--surface);
		color: var(--text);
		font-size: 13px;
		font-weight: 500;
		cursor: pointer;
	}

	.trigger:hover,
	.trigger[aria-expanded='true'] {
		background: var(--surface-hover);
	}

	.menu {
		position: fixed;
		z-index: 40;
		display: flex;
		flex-direction: column;
		width: min(240px, calc(100vw - 16px));
		padding: var(--space-1);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		box-shadow: var(--shadow);
		text-align: left;
	}

	.heading {
		margin: 0;
		padding: var(--space-2) var(--space-3) var(--space-1);
		font-size: 12px;
		font-weight: 600;
	}

	.menu button {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-height: 40px;
		padding: var(--space-1) var(--space-3);
		border: 0;
		border-radius: var(--radius-s);
		background: transparent;
		color: var(--text);
		text-align: left;
		cursor: pointer;
	}

	.menu button:hover,
	.menu button:focus-visible {
		background: var(--surface-hover);
		outline: none;
	}

	.menu button[aria-checked='true'] {
		color: var(--accent);
		font-weight: 500;
	}

	.label {
		flex: 1;
	}

	.order {
		font-size: 12px;
		font-weight: 400;
	}
</style>
