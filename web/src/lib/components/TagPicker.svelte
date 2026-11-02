<script lang="ts">
	// One tag type's button in the filter bar, opening a checklist of the library's tags of that
	// type, most used first, with a search that ignores case and accents, since a library can carry
	// thousands of genres (`requirements/tags.md` §7). It reads one page of the type's tags: a type
	// that fits in it is searched here, as each letter is typed, and a larger one by the server. It sits and closes as SortMenu does; on a
	// phone, where a panel beside the button would leave no room for the list, it is a sheet across
	// the bottom of the screen, closed from its own button or by tapping above it
	// (`requirements/general.md` §4).
	import { tick } from 'svelte';
	import { listTags } from '$lib/api/client';
	import type { Tag, TagPage, TagType } from '$lib/api/types';
	import type { TagMatch } from '$lib/filters';
	import { fullText } from '$lib/full-text';
	import Icon from './Icon.svelte';

	let {
		libraryId,
		type,
		label,
		chosen,
		ontoggle,
		variant = 'button',
		match,
		onmatch,
		onclear
	}: {
		libraryId: string;
		type: TagType;
		/** The type's name, such as `Genre`. */
		label: string;
		/** The IDs of the tags of this type the filter holds. */
		chosen: readonly string[];
		/** A tag checked or unchecked. */
		ontoggle: (tag: Tag) => void;
		/** `heading`: a column's heading in a list of songs; `button`: a button of its own. */
		variant?: 'button' | 'heading';
		/** Whether the filter's chosen tags of a type must all match, or any; neither for a mix. */
		match?: TagMatch;
		/** All or any chosen, offered once two of this type are. */
		onmatch?: (match: TagMatch) => void;
		/** This type's tags taken out of the filter. */
		onclear?: () => void;
	} = $props();

	/** At most this many tags are listed at once; the search finds the rest. */
	const SHOWN = 200;
	/** How many of the type's tags are read when it opens, the most a page holds. */
	const READ = 1000;

	let open = $state(false);
	/** The type's most used tags, every one of them where they fit in one page. */
	let first = $state<TagPage | null>(null);
	/** What the server last found for a search, where the type has more tags than one page. */
	let found = $state<TagPage | null>(null);
	let failed = $state(false);
	/** Counts the tries, so trying again searches again. */
	let attempt = $state(0);
	let query = $state('');
	let button = $state<HTMLButtonElement>();
	let panel = $state<HTMLElement>();
	let search = $state<HTMLInputElement>();
	let place = $state({ top: 0, left: 0 });
	/** Whether it opened as a sheet, on a screen as narrow as a phone's. */
	let sheet = $state(false);

	/** `text` as the search compares it: lowercase, with accents dropped. */
	const fold = (text: string) =>
		text
			.normalize('NFD')
			.replace(/\p{Mn}/gu, '')
			.toLowerCase();

	const wanted = $derived(query.trim());
	// Every tag of the type was read, so the search runs here.
	const complete = $derived(first !== null && first.nextCursor === null);
	/** The tags to list, and how many match in all; `null` until there are any. */
	const result = $derived.by((): { tags: Tag[]; total: number } | null => {
		if (!first) return null;
		if (complete || wanted === '') {
			const tags = first.items.filter((tag) => fold(tag.name).includes(fold(wanted)));
			return { tags, total: complete ? tags.length : first.total };
		}
		// The last search's tags stay listed until the next one's arrive.
		return found && { tags: found.items, total: found.total };
	});

	// A type with more tags than one page is searched by the server, once typing pauses.
	$effect(() => {
		if (!open || !first || complete || wanted === '') return;
		const q = wanted;
		void attempt;
		const timer = setTimeout(async () => {
			try {
				const page = await listTags(fetch, libraryId, {
					type: type.key,
					q,
					sort: 'trackCount',
					order: 'desc',
					limit: SHOWN
				});
				if (query.trim() === q) found = page;
			} catch {
				failed = true;
			}
		}, 150);
		return () => clearTimeout(timer);
	});

	/** Loads the type's most used tags the first time it opens. */
	async function load() {
		if (first) return;
		failed = false;
		try {
			first = await listTags(fetch, libraryId, {
				type: type.key,
				sort: 'trackCount',
				order: 'desc',
				limit: READ
			});
		} catch {
			failed = true;
		}
	}

	function retry() {
		failed = false;
		attempt++;
		void load();
	}

	async function show() {
		sheet = matchMedia('(max-width: 767px)').matches;
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
		if (!panel || !button || sheet) return;
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
	class={variant === 'heading' ? 'heading-trigger' : 'trigger'}
	class:active={chosen.length > 0}
	aria-haspopup="dialog"
	aria-expanded={open}
	aria-label="{variant === 'heading' ? `Filter by ${label.toLowerCase()}` : label}{chosen.length
		? `, ${chosen.length} chosen`
		: ''}"
	title={variant === 'heading' ? `Filter by ${label.toLowerCase()}` : undefined}
	onclick={() => (open ? close() : show())}
>
	{label}
	{#if chosen.length}<span class="count">{chosen.length}</span>{/if}
	<Icon name="chevron" size={14} />
</button>

{#if open}
	{#if sheet}<div class="backdrop" aria-hidden="true"></div>{/if}
	<div
		class="panel"
		class:sheet
		role="dialog"
		tabindex="-1"
		aria-label="{label} filter"
		bind:this={panel}
		style:top={sheet ? null : `${place.top}px`}
		style:left={sheet ? null : `${place.left}px`}
		{onkeydown}
	>
		{#if sheet}
			<header>
				<h2>{label}</h2>
				<button
					type="button"
					class="done"
					aria-label="Close {label.toLowerCase()} filter"
					onclick={close}
				>
					<Icon name="close" size={18} />
				</button>
			</header>
		{/if}
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
		{#if chosen.length && (onclear || (onmatch && chosen.length > 1))}
			<div class="chosen">
				{#if onmatch && chosen.length > 1}
					<div class="match" role="radiogroup" aria-label="Match">
						{#each [['any', 'Any'], ['all', 'All']] as const as [value, text] (value)}
							<button
								type="button"
								role="radio"
								aria-checked={match === value}
								onclick={() => onmatch(value)}>{text}</button
							>
						{/each}
					</div>
				{/if}
				{#if onclear}
					<button type="button" class="clear" onclick={onclear}>Clear</button>
				{/if}
			</div>
		{/if}
		{#if failed}
			<p class="note muted">
				Couldn't load the {label.toLowerCase()}s.
				<button type="button" class="retry" onclick={retry}>Try again</button>
			</p>
		{:else if !result}
			<p class="note muted">{first ? 'Searching…' : 'Loading…'}</p>
		{:else if result.tags.length === 0}
			<p class="note muted">No {label.toLowerCase()} matches “{query.trim()}”.</p>
		{:else}
			<ul aria-label="{label}s">
				{#each result.tags.slice(0, SHOWN) as tag (tag.id)}
					<li>
						<label class="tag">
							<input
								type="checkbox"
								checked={chosen.includes(tag.id)}
								onchange={() => ontoggle(tag)}
							/>
							<span class="name" {@attach fullText}>{tag.name}</span>
							<span class="tracks muted">{tag.trackCount.toLocaleString()}</span>
						</label>
					</li>
				{/each}
			</ul>
			{@const more = result.total - Math.min(result.tags.length, SHOWN)}
			{#if more > 0}
				<p class="note muted">{more.toLocaleString()} more. Search to find them.</p>
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

	/* As a column's heading: its text, with the heading's own colour until a tag is chosen. */
	.heading-trigger {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		min-width: 0;
		padding: 0;
		border: 0;
		border-radius: 4px;
		background: none;
		color: inherit;
		font: inherit;
		cursor: pointer;
	}

	.heading-trigger:hover,
	.heading-trigger[aria-expanded='true'] {
		color: var(--text);
	}

	.heading-trigger.active {
		color: var(--accent);
	}

	.heading-trigger:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	.chosen {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
		padding: var(--space-1) var(--space-2);
	}

	.match {
		display: inline-flex;
		border: 1px solid var(--border);
		border-radius: 14px;
		overflow: hidden;
	}

	.match button {
		height: 26px;
		padding: 0 var(--space-3);
		border: 0;
		background: var(--surface);
		color: var(--text);
		font-size: 12px;
		cursor: pointer;
	}

	.match button[aria-checked='true'] {
		background: var(--accent);
		color: var(--accent-text);
	}

	.clear {
		margin-left: auto;
		border: 0;
		background: none;
		color: var(--accent);
		font-size: 13px;
		cursor: pointer;
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

	/* On a phone: across the bottom, above everything but toasts, over a dimmed page. */
	.panel.sheet {
		right: 0;
		bottom: 0;
		left: 0;
		z-index: 19;
		width: 100%;
		max-height: 70dvh;
		padding: var(--space-2) var(--space-2) calc(var(--space-2) + env(safe-area-inset-bottom));
		border-width: 1px 0 0;
		border-radius: var(--radius-l) var(--radius-l) 0 0;
	}

	.backdrop {
		position: fixed;
		inset: 0;
		z-index: 18;
		background: rgb(0 0 0 / 0.35);
	}

	.sheet header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: var(--space-1) var(--space-2);
	}

	.sheet h2 {
		margin: 0;
		font-size: 16px;
	}

	.done {
		display: inline-flex;
		padding: var(--space-2);
		border: 0;
		border-radius: 50%;
		background: none;
		color: var(--text);
		cursor: pointer;
	}

	.sheet .tag {
		min-height: 44px;
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
