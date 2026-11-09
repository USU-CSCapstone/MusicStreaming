<script lang="ts">
	import { invalidateAll } from '$app/navigation';
	import { ApiError, adminListScans, startScan } from '$lib/api/client';
	import {
		TRIGGER_LABELS,
		findings,
		folder,
		progress,
		summarize,
		underWay,
		waiting,
		withPressed
	} from '$lib/api/scans';
	import type { Library, Scan } from '$lib/api/types';
	import Icon from '$lib/components/Icon.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import { plural } from '$lib/format';

	let { data } = $props();

	/** How often scans under way are asked after: often enough to watch, cheap for the server. */
	const POLL_MS = 1000;

	// Svelte 5 lets a derived be overwritten locally; it resets when the page reloads its data.
	let scans = $derived(data.scans);
	/** The libraries whose press of Scan now is on its way to the server. */
	let pressing = $state<Record<string, boolean>>({});
	let errors = $state<Record<string, string>>({});
	/** Each library's presses so far, so an answer asked for before the latest one is dropped. */
	const presses: Record<string, number> = {};

	/** A library's scans as shown: with a press on its way, a manual scan queued at once. */
	function shown(id: string): Scan[] {
		const list = scans[id] ?? [];
		return pressing[id] ? withPressed(list) : list;
	}

	/** Queues a scan of the whole library, shown queued at once (`requirements/general.md` §3.4). */
	async function scan(library: Library) {
		const id = library.id;
		presses[id] = (presses[id] ?? 0) + 1;
		pressing[id] = true;
		errors = Object.fromEntries(Object.entries(errors).filter(([key]) => key !== id));
		try {
			const started = await startScan(fetch, id);
			const list = scans[id] ?? [];
			scans = {
				...scans,
				[id]: list.some((s) => s.id === started.id)
					? list.map((s) => (s.id === started.id ? started : s))
					: [started, ...list]
			};
		} catch (e) {
			errors = { ...errors, [id]: message(e) };
		} finally {
			pressing[id] = false;
		}
	}

	// Asks again after each library with a scan under way while this page is open, and stops once
	// none is (`requirements/scanning.md` §5). A failed ask is tried again at the next.
	$effect(() => {
		const ids = data.libraries
			.map((l) => l.id)
			.filter((id) => !pressing[id] && underWay(scans[id] ?? []));
		if (ids.length === 0) return;
		const timer = setTimeout(() => void poll(ids), POLL_MS);
		return () => clearTimeout(timer);
	});

	async function poll(ids: string[]) {
		const asked = ids.map((id) => [id, presses[id] ?? 0] as const);
		const answers = await Promise.all(
			asked.map(([id]) => adminListScans(fetch, id).catch(() => null))
		);
		const next = { ...scans };
		let finished = false;
		asked.forEach(([id, press], i) => {
			const answer = answers[i];
			// A press since replaces what this ask found.
			if (!answer || pressing[id] || (presses[id] ?? 0) !== press) return;
			finished ||= !underWay(answer.items);
			next[id] = answer.items;
		});
		scans = next;
		// What a scan found changes the library's counts, here and in the sidebar.
		if (finished) await invalidateAll();
	}

	function message(e: unknown): string {
		return e instanceof ApiError ? e.message : 'Something went wrong. Try again.';
	}

	/** How the last scan ended, and what it found. */
	function ended(last: Scan | null): string {
		if (!last?.finishedAt) return 'Not scanned yet';
		const at = new Date(last.finishedAt).toLocaleString();
		const found = findings(last);
		if (last.state === 'cancelled') return `The last scan was cancelled, ${at}`;
		if (last.state === 'suspended') {
			return `The last scan stopped short, ${at}${found ? `: ${found}` : ''}`;
		}
		return `Last scanned ${at}: ${found || 'nothing changed'}`;
	}

	/** The scans waiting to start, in order: "Manual scan, then Scan of changed files". */
	function inOrder(queued: Scan[]): string {
		return queued.map((s) => TRIGGER_LABELS[s.trigger]).join(', then ');
	}
</script>

<PageHeader title="Libraries" subtitle="Your music, and how Jewelcase keeps up with it." />

<section aria-label="Libraries">
	{#if data.libraries.length === 0}
		<p class="empty muted">No libraries yet.</p>
	{:else}
		<ul class="libraries">
			{#each data.libraries as library (library.id)}
				{@const list = shown(library.id)}
				{@const { running, queued, last } = summarize(list)}
				{@const queuedAlready = waiting(list) !== null}
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
						<!-- A scan of the whole library already queued is what another press would get. -->
						<button
							type="button"
							onclick={() => scan(library)}
							disabled={queuedAlready}
							aria-label="{queuedAlready ? 'Queued' : 'Scan now'}: {library.name}"
						>
							{queuedAlready ? 'Queued' : 'Scan now'}
						</button>
					</div>
					{#if running}
						{@const where = folder(running)}
						{@const found = findings(running)}
						<div class="running">
							<p>
								<span class="dot" aria-hidden="true"></span>
								<span><strong>{TRIGGER_LABELS[running.trigger]}</strong> · {progress(running)}</span
								>
							</p>
							{#if where}<p class="muted small where">In {where}</p>{/if}
							{#if found}<p class="muted small">So far: {found}</p>{/if}
						</div>
					{/if}
					{#if queued.length}
						<p class="muted small">
							{running ? 'Up next' : 'About to start'}: {inOrder(queued)}
						</p>
					{/if}
					{#if !running && !queued.length}
						<p class="muted small">{ended(last)}</p>
					{/if}
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

	.running {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
	}

	.running p:first-child {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	/* Pulses while the scan runs; still for anyone who asks for less motion. */
	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--accent);
		animation: pulse 1.2s ease-in-out infinite;
	}

	@keyframes pulse {
		50% {
			opacity: 0.3;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.dot {
			animation: none;
		}
	}

	.where {
		overflow-wrap: anywhere;
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
