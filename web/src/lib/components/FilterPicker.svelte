<script lang="ts">
	// One tag type's part of a list's filter: its tags, and whether an item must carry all of them
	// or any, which puts them in the filter's `all` or `any` (`requirements/tags.md` §7).
	import { page } from '$app/state';
	import type { TagType } from '$lib/api/types';
	import {
		type TagFilter,
		chosenTags,
		matchTags,
		toggleTag,
		typeMatch,
		withoutTags
	} from '$lib/filters';
	import { knownTags } from '$lib/known-tags.svelte';
	import { tagTypeName, tagTypePlural } from '$lib/tags';
	import TagPicker from './TagPicker.svelte';

	let {
		libraryId,
		type,
		variant,
		filter,
		onfilter
	}: {
		libraryId: string;
		type: TagType;
		variant: 'button' | 'heading';
		/** The filter as it shows, the one chosen while its load has not finished (`shownFilter`). */
		filter: TagFilter;
		onfilter: (filter: TagFilter) => void;
	} = $props();

	$effect(() => {
		if (libraryId) knownTags.load(libraryId, chosenTags(filter));
	});
	// The type's chosen tags, in the order chosen: those read as of this type.
	const chosen = $derived(
		chosenTags(filter).filter((id) => knownTags.get(libraryId, id)?.type === type.key)
	);
</script>

<TagPicker
	{libraryId}
	{type}
	{variant}
	label={tagTypeName(type.key, page.data.tagTypes)}
	labels={tagTypePlural(type.key, page.data.tagTypes)}
	{chosen}
	match={typeMatch(filter, chosen)}
	ontoggle={(tag) => {
		knownTags.note(libraryId, tag);
		onfilter(toggleTag(filter, tag.id, chosen));
	}}
	onmatch={(match) => onfilter(matchTags(filter, chosen, match))}
	onclear={() => onfilter(withoutTags(filter, chosen))}
/>
