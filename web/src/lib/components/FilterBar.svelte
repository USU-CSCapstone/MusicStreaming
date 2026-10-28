<script lang="ts">
	// A list's tag filter: a button for each type the library's music carries, the tags chosen as
	// chips, and whether a type's chosen tags must all match (`requirements/tags.md` §2, §7). Types
	// come from the library, so one a plugin adds gets a button with no change here, and a library
	// with no tags shows nothing.
	import { SvelteMap } from 'svelte/reactivity';
	import { getTag, listTagTypes } from '$lib/api/client';
	import type { Tag, TagType } from '$lib/api/types';
	import { type TagFilter, UNFILTERED, isFiltered, toggleTag } from '$lib/filters';
	import Icon from './Icon.svelte';
	import TagPicker from './TagPicker.svelte';

	let {
		libraryId,
		filter,
		onchange
	}: {
		libraryId: string;
		filter: TagFilter;
		/** The filter as the user changed it. */
		onchange: (filter: TagFilter) => void;
	} = $props();

	/** The built-in types' names; any other type is named after its key. */
	const NAMES: Record<string, string> = { genre: 'Genre', mood: 'Mood' };
	const typeName = (key: string) => NAMES[key] ?? key.charAt(0).toUpperCase() + key.slice(1);

	let types = $state<TagType[]>([]);
	/** What is known of each chosen tag; `null` for one the library does not have. */
	const known = new SvelteMap<string, Pick<Tag, 'name' | 'type'> | null>();

	$effect(() => {
		const library = libraryId;
		listTagTypes(fetch, library)
			.then((loaded) => {
				if (library === libraryId) types = loaded;
			})
			.catch(() => {
				types = [];
			});
	});

	// A filter can arrive in the URL before any of its tags has been listed, so each chosen tag
	// not yet known is read once.
	$effect(() => {
		for (const id of filter.tags) {
			if (known.has(id)) continue;
			known.set(id, { name: '…', type: '' });
			getTag(fetch, libraryId, id)
				.then((tag) => known.set(id, { name: tag.name, type: tag.type }))
				.catch(() => known.set(id, null));
		}
	});

	const chosenOf = (key: string) => filter.tags.filter((id) => known.get(id)?.type === key);

	/** Whether a type has two tags chosen, so whether they must all match is a choice to show. */
	const severalOfOneType = $derived(types.some((type) => chosenOf(type.key).length > 1));

	function toggle(tag: Tag) {
		known.set(tag.id, { name: tag.name, type: tag.type });
		onchange(toggleTag(filter, tag.id));
	}
</script>

{#if types.length || isFiltered(filter)}
	<div class="filters" role="group" aria-label="Filters">
		{#each types as type (type.key)}
			<TagPicker
				{libraryId}
				{type}
				label={typeName(type.key)}
				chosen={chosenOf(type.key)}
				ontoggle={toggle}
			/>
		{/each}

		{#each filter.tags as id (id)}
			{@const tag = known.get(id)}
			{@const name = tag === null ? 'Unknown tag' : (tag?.name ?? '…')}
			{@const kind = tag?.type ? typeName(tag.type).toLowerCase() : 'tag'}
			<span class="chip" class:unknown={tag === null}>
				{name}
				<button
					type="button"
					aria-label="Remove {kind} {name}"
					onclick={() => onchange(toggleTag(filter, id))}
				>
					<Icon name="close" size={14} />
				</button>
			</span>
		{/each}

		{#if severalOfOneType}
			<div class="match" role="radiogroup" aria-label="Of each type, match">
				{#each [['any', 'Any'], ['all', 'All']] as const as [match, text] (match)}
					<button
						type="button"
						role="radio"
						aria-checked={filter.match === match}
						onclick={() => onchange({ ...filter, match })}
					>
						{text}
					</button>
				{/each}
			</div>
		{/if}

		{#if isFiltered(filter)}
			<button type="button" class="clear" onclick={() => onchange({ ...UNFILTERED })}>
				Clear filters
			</button>
		{/if}
	</div>
{/if}

<style>
	.filters {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		height: 28px;
		padding: 0 var(--space-1) 0 var(--space-3);
		border-radius: 14px;
		background: var(--accent-soft);
		color: var(--text);
		font-size: 13px;
	}

	.chip.unknown {
		background: var(--surface-hover);
		color: var(--text-muted);
	}

	.chip button {
		display: inline-flex;
		padding: 3px;
		border: 0;
		border-radius: 50%;
		background: none;
		color: inherit;
		cursor: pointer;
	}

	.chip button:hover,
	.chip button:focus-visible {
		background: var(--surface-hover);
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
		border: 0;
		background: none;
		color: var(--accent);
		font-size: 13px;
		cursor: pointer;
	}
</style>
