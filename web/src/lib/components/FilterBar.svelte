<script lang="ts">
	// A list's tag filter: a button for each type the library's music carries, the tags chosen as
	// chips, and whether a type's chosen tags must all match (`requirements/tags.md` §2, §7). Types
	// come from the library, so one a plugin adds gets a button with no change here, and a library
	// with no tags shows nothing.
	import { listTagTypes } from '$lib/api/client';
	import type { Tag, TagType } from '$lib/api/types';
	import { type TagFilter, UNFILTERED, isFiltered, toggleTag } from '$lib/filters';
	import { knownTags } from '$lib/known-tags.svelte';
	import { tagTypeName } from '$lib/tags';
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

	let types = $state<TagType[]>([]);

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
	// not yet known is read, once for the whole app.
	$effect(() => knownTags.load(libraryId, filter.tags));

	const known = (id: string) => knownTags.get(libraryId, id);
	const chosenOf = (key: string) => filter.tags.filter((id) => known(id)?.type === key);

	/** Whether a type has two tags chosen, so whether they must all match is a choice to show. */
	const severalOfOneType = $derived(types.some((type) => chosenOf(type.key).length > 1));

	function toggle(tag: Tag) {
		knownTags.note(libraryId, tag);
		onchange(toggleTag(filter, tag.id));
	}
</script>

{#if types.length || isFiltered(filter)}
	<div class="filters" role="group" aria-label="Filters">
		{#each types as type (type.key)}
			<TagPicker
				{libraryId}
				{type}
				label={tagTypeName(type.key)}
				chosen={chosenOf(type.key)}
				ontoggle={toggle}
			/>
		{/each}

		{#each filter.tags as id (id)}
			{@const tag = known(id)}
			{@const name = tag === null ? 'Unknown tag' : (tag?.name ?? '…')}
			{@const kind = tag?.type ? tagTypeName(tag.type).toLowerCase() : 'tag'}
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
	/* It sits under the page's header, closer to it than the header's own margin. */
	.filters {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
		margin: calc(-1 * var(--space-2)) 0 var(--space-4);
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
