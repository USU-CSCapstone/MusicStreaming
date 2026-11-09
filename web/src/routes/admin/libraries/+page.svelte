<script lang="ts">
	import { ApiError, adminListScans, startScan } from '$lib/api/client';
	import { TRIGGER_LABELS, summarize } from '$lib/api/scans';
	import type { Library, Scan } from '$lib/api/types';
	import Icon from '$lib/components/Icon.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { plural } from '$lib/format';

	let { data } = $props();

	// Svelte 5 lets a derived be overwritten locally; it resets when the page reloads its data.
	let scans = $derived(data.scans);
	let starting = $state<Record<string, boolean>>({});
	let errors = $state<Record<string, string>>({});

	/** Queues a scan of the whole library, then shows where its scans stand. */
	async function scan(library: Library) {
		const id = library.id;
		starting[id] = true;
		errors = Object.fromEntries(Object.entries(errors).filter(([key]) => key !== id));
		try {
			await startScan(fetch, id);
			scans = { ...scans, [id]: (await adminListScans(fetch, id)).items };
		} catch (e) {
			errors = { ...errors, [id]: message(e) };
		} finally {
			starting[id] = false;
		}
	}

	function message(e: unknown): string {
		return e instanceof ApiError ? e.message : 'Something went wrong. Try again.';
	}

	/** Where the library's scans stand, in a line. */
	function status(list: Scan[]): string {
		const { running, next, last } = summarize(list);
		if (running) return `${TRIGGER_LABELS[running.trigger]} running`;
		if (next) return `${TRIGGER_LABELS[next.trigger]} about to start`;
		if (!last?.finishedAt) return 'Not scanned yet';
		const at = new Date(last.finishedAt).toLocaleString();
		if (last.state === 'suspended') return `The last scan stopped short, ${at}`;
		if (last.state === 'cancelled') return `The last scan was cancelled, ${at}`;
		return `Last scanned ${at}`;
	}
</script>

<PageHeader title="Libraries" subtitle="Your music, and how Jewelcase keeps up with it." />

<section aria-label="Libraries">
	{#if data.libraries.length === 0}
		<p class="empty muted">No libraries yet.</p>
	{:else}
		<ul class="libraries">
			{#each data.libraries as library (library.id)}
				<li class="card">
					<div class="top">
						<span class="icon"><Icon name="folder" size={20} /></span>
						<div class="meta">
							<h3>{library.name}</h3>
							<p class="muted small">
								{plural(library.trackCount, 'song')} · {plural(library.albumCount, 'album')} ·
								{plural(library.artistCount, 'artist')}
							</p>
						</div>
						<button
							type="button"
							onclick={() => scan(library)}
							disabled={starting[library.id]}
							aria-label="Scan {library.name} now"
						>
							{starting[library.id] ? 'Starting…' : 'Scan now'}
						</button>
					</div>
					<p class="muted small">{status(scans[library.id] ?? [])}</p>
					{#if errors[library.id]}<p class="error" role="alert">{errors[library.id]}</p>{/if}
				</li>
			{/each}
		</ul>
	{/if}
</section>

<style>
	section {
		max-width: 760px;
	}

	.libraries {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.card {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding: var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-l);
		background: var(--surface);
	}

	.top {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.icon {
		display: grid;
		place-items: center;
		flex: none;
		width: 40px;
		height: 40px;
		border-radius: var(--radius-m);
		background: var(--accent-soft);
		color: var(--accent);
	}

	.meta {
		flex: 1;
		min-width: 0;
	}

	h3 {
		font-size: 16px;
		overflow-wrap: anywhere;
	}

	p {
		margin: 0;
	}

	.small {
		font-size: 13px;
	}

	button {
		flex: none;
		height: 34px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 17px;
		background: var(--accent);
		color: var(--accent-text);
		font-weight: 600;
		cursor: pointer;
	}

	button:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.error {
		color: #dc2626;
	}

	.empty {
		margin: 0;
	}

	@media (prefers-color-scheme: dark) {
		.error {
			color: #f87171;
		}
	}
</style>
