<script lang="ts">
	import { resolve } from '$app/paths';
	import type { TagsByType } from '$lib/api/types';
	import { tagTypeName } from '$lib/tags';

	// An album's or artist's tags, each a link to its page (`requirements/conventions.md` §3,
	// `requirements/tags.md` §7), a type's together in name order and the types in key order.
	let { tags }: { tags: TagsByType } = $props();

	const types = $derived(Object.keys(tags).sort());
</script>

<span class="tags">
	{#each types as type, t (type)}
		<!-- Svelte trims a separator's leading space, so it is a no-break one; the line break after it is the space after. -->
		{#if t > 0}<span class="between" aria-hidden="true">&nbsp;·</span>{/if}
		<span class="type" aria-label={tagTypeName(type)} role="group">
			{#each tags[type] as tag, i (tag.id)}
				{#if i > 0}<span class="sep">, </span>{/if}
				<a href={resolve('/tags/[id]', { id: tag.id })}>{tag.name}</a>
			{/each}
		</span>
	{/each}
</span>

<style>
	.tags {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
