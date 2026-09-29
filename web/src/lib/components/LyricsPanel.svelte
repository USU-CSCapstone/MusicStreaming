<script lang="ts">
	import { page } from '$app/state';
	import { getLyrics } from '$lib/api/client';
	import type { Lyrics } from '$lib/api/types';
	import { artistName } from '$lib/format';
	import { panels } from '$lib/panels.svelte';
	import { player } from '$lib/player.svelte';
	import Icon from './Icon.svelte';

	const track = $derived(player.current);
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
		const ms = player.position * 1000;
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

<svelte:window
	onkeydown={(e) => {
		if (panels.lyrics && e.key === 'Escape') panels.lyrics = false;
	}}
/>

{#if panels.lyrics}
	<aside class="panel" aria-label="Lyrics">
		<header>
			<div class="meta">
				<h2>Lyrics</h2>
				{#if track}
					<p class="muted">
						{track.title} · {track.artists.map((a) => artistName(a.name)).join(', ')}
					</p>
				{/if}
			</div>
			<button type="button" aria-label="Close lyrics" onclick={() => (panels.lyrics = false)}>
				<Icon name="close" size={18} />
			</button>
		</header>

		<div class="body">
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
								onclick={() => player.seek(line.startMs / 1000)}
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
		</div>
	</aside>
{/if}

<style>
	.panel {
		position: absolute;
		inset: 0 0 0 auto;
		z-index: 10;
		display: flex;
		flex-direction: column;
		width: min(400px, 100%);
		background: var(--surface);
		border-left: 1px solid var(--border);
		box-shadow: var(--shadow);
	}

	header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-4) var(--space-3) var(--space-5);
		border-bottom: 1px solid var(--border);
	}

	.meta {
		min-width: 0;
	}

	h2 {
		font-size: 18px;
	}

	.meta p {
		margin: 2px 0 0;
		font-size: 13px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	header button {
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
		flex-shrink: 0;
	}

	header button:hover {
		background: var(--surface-hover);
		color: var(--text);
	}

	.body {
		flex: 1;
		overflow-y: auto;
		padding: var(--space-4) var(--space-5) var(--space-6);
	}

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
