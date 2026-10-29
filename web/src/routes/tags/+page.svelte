<script lang="ts">
	import { resolve } from '$app/paths';
	import { listTags } from '$lib/api/client';
	import Icon from '$lib/components/Icon.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { plural } from '$lib/format';
	import { Paged } from '$lib/paged.svelte';
	import { tagTypeName, tagTypePlural } from '$lib/tags';

	// One type's tags, each a link to its page (`requirements/tags.md` §7). A messy library shows
	// as messy: near-duplicates are listed as they are (§4).
	let { data } = $props();

	const tags = $derived(
		new Paged(data.tags, (cursor) => listTags(fetch, data.lib, { ...data.query, cursor }))
	);
	const name = $derived(tagTypeName(data.type).toLowerCase());
</script>

<svelte:head><title>{tagTypePlural(data.type)} · Jewelcase</title></svelte:head>

<PageHeader title={tagTypePlural(data.type)} subtitle={plural(tags.total, name)} />

{#if tags.items.length}
	<ul class="tags">
		{#each tags.items as tag (tag.id)}
			<li>
				<a href={resolve('/tags/[id]', { id: tag.id })}>
					<span class="icon"><Icon name="tag" size={18} /></span>
					<span class="name">{tag.name}</span>
					<span class="count muted">{plural(tag.trackCount, 'song')}</span>
				</a>
			</li>
		{/each}
	</ul>
	<div {@attach tags.sentinel} aria-hidden="true"></div>
{:else}
	<p class="muted">No {name}s yet. They appear here as the library's files are scanned.</p>
{/if}

<style>
	.tags {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
		gap: var(--space-2);
		margin: 0;
		padding: 0;
		list-style: none;
	}

	a {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-3);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		text-decoration: none;
	}

	a:hover,
	a:focus-visible {
		background: var(--surface-hover);
	}

	.icon {
		display: inline-flex;
		color: var(--accent);
	}

	.name {
		flex: 1;
		overflow: hidden;
		font-weight: 500;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.count {
		font-size: 13px;
		white-space: nowrap;
	}
</style>
