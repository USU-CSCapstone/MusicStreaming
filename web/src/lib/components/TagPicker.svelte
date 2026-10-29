<script lang="ts">
	// One tag type's button in the filter bar, opening a checklist of the library's tags of that
	// type, most used first, with a search that ignores case and accents, since a library can carry
	// thousands of genres (`requirements/tags.md` §7). It sits and closes as SortMenu does.
	import { tick } from 'svelte';
	import { all, listTags } from '$lib/api/client';
	import type { Tag, TagType } from '$lib/api/types';
	import Icon from './Icon.svelte';

	let {
		libraryId,
		type,
		label,
		chosen,
		ontoggle
	}: {
		libraryId: string;
		type: TagType;
		/** The type's name, such as `Genre`. */
		label: string;
		/** The IDs of the tags of this type the filter holds. */
		chosen: readonly string[];
		/** A tag checked or unchecked. */
		ontoggle: (tag: Tag) => void;
	} = $props();

	/** At most this many tags are listed at once; the search finds the rest. */
	const SHOWN = 200;

	let open = $state(false);
	let tags = $state<Tag[] | null>(null);
	let failed = $state(false);
	let query = $state('');
	let button = $state<HTMLButtonElement>();
	let panel = $state<HTMLElement>();
	let search = $state<HTMLInputElement>();
	let place = $state({ top: 0, left: 0 });

	/** `text` as the search compares it: lowercase, with accents dropped. */
	const fold = (text: string) =>
		text
			.normalize('NFD')
			.replace(/\p{Mn}/gu, '')
			.toLowerCase();

	const matching = $derived.by(() => {
		const wanted = fold(query.trim());
		return (tags ?? []).filter((tag) => fold(tag.name).includes(wanted));
	});

	/** Loads the type's tags the first time it opens. */
	async function load() {
		if (tags) return;
		failed = false;
		try {
			tags = await all((cursor) =>
				listTags(fetch, libraryId, {
					type: type.key,
					sort: 'trackCount',
					order: 'desc',
					limit: 1000,
					cursor
				})
			);
		} catch {
			failed = true;
		}
	}

	async function show() {
		open = true;
		await tick();
		position();
		search?.focus();
		await load();
		position();
	}

	/** Focus goes back to the button. */
	function close() {
		open = false;
		query = '';
		button?.focus();
	}

	/** Under the button, lined up on the left, or above it when there is no room below. */
	function position() {
		if (!panel || !button) return;
		const a = button.getBoundingClientRect();
		const { width, height } = panel.getBoundingClientRect();
		const below = a.bottom + 4;
		const top = below + height > innerHeight - 8 ? Math.max(8, a.top - 4 - height) : below;
		const left = Math.min(Math.max(8, a.left), innerWidth - width - 8);
		place = { top, left };
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape') return;
		e.preventDefault();
		e.stopPropagation();
		close();
	}

	// It belongs beside its button, so it goes when a press lands elsewhere or the page scrolls.
	$effect(() => {
		if (!open) return;
		const away = (e: Event) => {
			const target = e.target as Node;
			if (!panel?.contains(target) && !button?.contains(target)) open = false;
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
	class:active={chosen.length > 0}
	aria-haspopup="dialog"
	aria-expanded={open}
	aria-label={chosen.length ? `${label}, ${chosen.length} chosen` : label}
	onclick={() => (open ? close() : show())}
>
	{label}
	{#if chosen.length}<span class="count">{chosen.length}</span>{/if}
	<Icon name="chevron" size={14} />
</button>

{#if open}
	<div
		class="panel"
		role="dialog"
		tabindex="-1"
		aria-label="{label} filter"
		bind:this={panel}
		style:top="{place.top}px"
		style:left="{place.left}px"
		{onkeydown}
	>
		<label class="search">
			<Icon name="search" size={16} />
			<input
				bind:this={search}
				bind:value={query}
				type="search"
				placeholder="Find a {label.toLowerCase()}"
				aria-label="Find a {label.toLowerCase()}"
			/>
		</label>
		{#if failed}
			<p class="note muted">
				Couldn't load the {label.toLowerCase()}s.
				<button type="button" class="retry" onclick={load}>Try again</button>
			</p>
		{:else if !tags}
			<p class="note muted">Loading…</p>
		{:else if matching.length === 0}
			<p class="note muted">No {label.toLowerCase()} matches “{query.trim()}”.</p>
		{:else}
			<ul aria-label="{label}s">
				{#each matching.slice(0, SHOWN) as tag (tag.id)}
					<li>
						<label class="tag">
							<input
								type="checkbox"
								checked={chosen.includes(tag.id)}
								onchange={() => ontoggle(tag)}
							/>
							<span class="name">{tag.name}</span>
							<span class="tracks muted">{tag.trackCount.toLocaleString()}</span>
						</label>
					</li>
				{/each}
			</ul>
			{#if matching.length > SHOWN}
				<p class="note muted">
					{(matching.length - SHOWN).toLocaleString()} more. Search to find them.
				</p>
			{/if}
		{/if}
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

	.trigger.active {
		border-color: var(--accent);
		color: var(--accent);
	}

	.count {
		min-width: 18px;
		padding: 0 5px;
		border-radius: 9px;
		background: var(--accent);
		color: var(--accent-text);
		font-size: 11px;
		line-height: 18px;
		text-align: center;
	}

	.panel {
		position: fixed;
		z-index: 40;
		display: flex;
		flex-direction: column;
		width: min(280px, calc(100vw - 16px));
		max-height: min(420px, calc(100vh - 16px));
		padding: var(--space-1);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		box-shadow: var(--shadow);
	}

	.search {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin: var(--space-1);
		padding: 0 var(--space-2);
		border: 1px solid var(--border);
		border-radius: var(--radius-s);
		color: var(--text-muted);
	}

	.search input {
		flex: 1;
		min-width: 0;
		height: 32px;
		border: 0;
		background: transparent;
		color: var(--text);
		font: inherit;
		outline: none;
	}

	ul {
		overflow-y: auto;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.tag {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		min-height: 36px;
		padding: var(--space-1) var(--space-2);
		border-radius: var(--radius-s);
		cursor: pointer;
	}

	.tag:hover,
	.tag:focus-within {
		background: var(--surface-hover);
	}

	.name {
		flex: 1;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.tracks {
		font-size: 12px;
		font-variant-numeric: tabular-nums;
	}

	.note {
		margin: 0;
		padding: var(--space-2) var(--space-3);
		font-size: 13px;
	}

	.retry {
		border: 0;
		background: none;
		color: var(--accent);
		font: inherit;
		cursor: pointer;
	}
</style>
