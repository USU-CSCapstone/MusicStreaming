<script lang="ts">
	import { page } from '$app/state';
	import { imageUrl } from '$lib/api/client';
	import type { ImageRef } from '$lib/api/types';
	import Icon, { type IconName } from './Icon.svelte';

	let {
		image,
		seed,
		size,
		icon = 'album',
		round = false,
		alt = ''
	}: {
		image: ImageRef | null | undefined;
		/** Picks the placeholder's colours, so each entity's stand-in is stable. */
		seed: string;
		/** The displayed size in CSS pixels; picks the resolution fetched. */
		size: number;
		icon?: IconName;
		round?: boolean;
		alt?: string;
	} = $props();

	let loaded = $state(false);
	let failed = $state(false);
	const libraryId = $derived(page.data.library?.id ?? '');
	const src = $derived(image && libraryId ? imageUrl(libraryId, image, size) : null);
	const hue = $derived([...seed].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % 360);

	$effect(() => {
		void src;
		loaded = false;
		failed = false;
	});
</script>

<!-- Missing art is the common case in a fresh library, so the stand-in is designed, not broken
     (`requirements/conventions.md` §5). -->
<div class="art" class:round style:--hue={hue}>
	{#if !loaded}
		<div class="placeholder">
			<Icon name={icon} size={Math.max(16, Math.min(56, size / 3))} />
		</div>
	{/if}
	{#if src && !failed}
		<img
			{src}
			{alt}
			loading="lazy"
			decoding="async"
			class:loaded
			onload={() => (loaded = true)}
			onerror={() => (failed = true)}
		/>
	{/if}
</div>

<style>
	.art {
		position: relative;
		width: 100%;
		aspect-ratio: 1;
		border-radius: var(--radius-s);
		overflow: hidden;
		background: var(--surface-2);
		flex-shrink: 0;
	}

	.round {
		border-radius: 50%;
	}

	.placeholder {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		color: hsl(var(--hue) 40% 92% / 0.75);
		background: linear-gradient(
			135deg,
			hsl(var(--hue) 42% 58%),
			hsl(calc(var(--hue) + 45) 38% 38%)
		);
	}

	img {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
		opacity: 0;
		transition: opacity 120ms ease-out;
	}

	img.loaded {
		opacity: 1;
	}
</style>
