<script lang="ts">
	import { page } from '$app/state';
	import { getLyrics } from '$lib/api/client';
	import type { Lyrics } from '$lib/api/types';
	import { artistName } from '$lib/format';
	import { panels } from '$lib/panels.svelte';
	import { controls } from '$lib/controls.svelte';
	import Panel from './Panel.svelte';

	// The track the player bar shows, here or on another device.
	const track = $derived(controls.track);
	let lyrics = $state<Lyrics | null>(null);
	let failed = $state(false);
	let list = $state<HTMLOListElement>();

	// Fetched when the panel opens or the track changes. Most tracks have none, and that
	// is a clean empty state, never an error (`requirements/tracks.md` §5).
	$effect(() => {
		const t = track;
		const libraryId = page.data.library?.id;
		if (!panels.lyrics || !t || !libraryId) return;
		let current = true;
		lyrics = null;
		failed = false;
		getLyrics(fetch, libraryId, t.id)
			.then((l) => current && (lyrics = l))
			.catch(() => current && (failed = true));
		return () => {
			current = false;
		};
	});

	const lines = $derived(lyrics?.kind === 'synced' ? (lyrics.lines ?? []) : []);

	/** The line being sung: the last one that has started. Stays right through seeks and pauses. */
	const now = $derived.by(() => {
		const ms = controls.position * 1000;
		let at = -1;
		for (let i = 0; i < lines.length && lines[i].startMs <= ms; i++) at = i;
		return at;
	});

	$effect(() => {
		const el = now >= 0 ? list?.children[now] : undefined;
		if (!el) return;
		const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
		el.scrollIntoView({ block: 'center', behavior: reduced ? 'auto' : 'smooth' });
	});
</script>

{#if panels.lyrics}
	<Panel
		title="Lyrics"
		subtitle={track
			? `${track.title} · ${track.artists.map((a) => artistName(a.name)).join(', ')}`
			: undefined}
		onclose={() => (panels.lyrics = false)}
	>
		{#if !track}
			<p class="state muted">Play something to see its lyrics.</p>
		{:else if failed}
			<p class="state muted">Lyrics could not be loaded.</p>
		{:else if !lyrics}
			<p class="state muted" aria-busy="true">Loading…</p>
		{:else if lyrics.kind === 'synced'}
			<ol bind:this={list} class="synced">
				{#each lines as line, i (i)}
					<li>
						<!-- Clicking a line seeks there. -->
						<button
							type="button"
							class:current={i === now}
							class:past={i < now}
							aria-current={i === now ? 'true' : undefined}
							onclick={() => controls.seek(line.startMs / 1000)}
						>
							{line.text || '♪'}
						</button>
					</li>
				{/each}
			</ol>
		{:else if lyrics.kind === 'plain'}
			<p class="plain">{lyrics.plain}</p>
		{:else}
			<p class="state muted">No lyrics for this song.</p>
		{/if}
	</Panel>
{/if}

<style>
	.state {
		margin: var(--space-6) 0 0;
		text-align: center;
	}

	.synced {
		list-style: none;
		margin: 0;
		padding: 30% 0;
	}

	.synced button {
		all: unset;
		display: block;
		width: 100%;
		padding: 6px var(--space-2);
		margin: 0 calc(-1 * var(--space-2));
		border-radius: var(--radius-s);
		font-size: 20px;
		font-weight: 600;
		line-height: 1.35;
		color: var(--text-faint);
		cursor: pointer;
		transition: color 150ms ease-out;
	}

	.synced button:hover {
		background: var(--surface-hover);
	}

	.synced button:focus-visible {
		outline: 2px solid var(--accent);
	}

	.synced button.past {
		color: var(--text-muted);
	}

	.synced button.current {
		color: var(--text);
	}

	.plain {
		margin: 0;
		white-space: pre-line;
		font-size: 16px;
		line-height: 1.6;
	}

	@media (prefers-reduced-motion: reduce) {
		.synced button {
			transition: none;
		}
	}
</style>
