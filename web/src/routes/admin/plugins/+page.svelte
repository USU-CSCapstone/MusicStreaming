<script lang="ts">
	import {
		ApiError,
		installPluginFile,
		installPluginUrl,
		setPluginEnabled,
		setPluginPermissions,
		uninstallPlugin
	} from '$lib/api/client';
	import { PERMISSION_LABELS, isLibraryPermission, type Plugin } from '$lib/api/plugins';
	import Icon from '$lib/components/Icon.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import PermissionDialog, { type Decision } from '$lib/components/PermissionDialog.svelte';

	let { data } = $props();

	// Svelte 5 lets a derived be overwritten locally; it resets when the page reloads its data.
	let plugins = $derived(data.plugins);
	const libraries = $derived(
		data.library ? [{ id: data.library.id, name: data.library.name }] : []
	);
	const libraryName = (id: string) => libraries.find((l) => l.id === id)?.name ?? 'this library';

	let installing = $state(false);
	let installError = $state<string | null>(null);
	let url = $state('');
	let dragging = $state(false);
	let fileInput: HTMLInputElement;

	let dialog = $state<{ plugin: Plugin; mode: 'install' | 'edit' } | null>(null);
	let dialogBusy = $state(false);
	let dialogError = $state<string | null>(null);
	let confirming = $state<string | null>(null);
	let rowError = $state<Record<string, string>>({});

	function message(e: unknown): string {
		return e instanceof ApiError ? e.message : 'Something went wrong. Try again.';
	}

	function replace(p: Plugin) {
		plugins = plugins.some((x) => x.id === p.id)
			? plugins.map((x) => (x.id === p.id ? p : x))
			: [...plugins, p].sort((a, b) => a.name.localeCompare(b.name));
	}

	async function install(run: () => Promise<Plugin>) {
		installing = true;
		installError = null;
		try {
			const p = await run();
			replace(p);
			dialogError = null;
			dialog = { plugin: p, mode: 'install' };
			url = '';
		} catch (e) {
			installError = message(e);
		} finally {
			installing = false;
		}
	}

	function pickFile(files: FileList | null | undefined) {
		const file = files?.[0];
		if (file) void install(() => installPluginFile(fetch, file));
		if (fileInput) fileInput.value = '';
	}

	async function approve(decision: Decision) {
		if (!dialog) return;
		const id = dialog.plugin.id;
		dialogBusy = true;
		dialogError = null;
		try {
			let p = await setPluginPermissions(fetch, id, decision.grants);
			for (const lib of p.libraries) {
				const want = decision.enable[lib.libraryId] ?? lib.enabled;
				if (want !== lib.enabled) p = await setPluginEnabled(fetch, id, lib.libraryId, want);
			}
			replace(p);
			dialog = null;
		} catch (e) {
			dialogError = message(e);
		} finally {
			dialogBusy = false;
		}
	}

	async function cancel() {
		if (!dialog) return;
		// Cancelling a fresh install removes it, so nothing is left half-installed.
		if (dialog.mode === 'install') {
			const id = dialog.plugin.id;
			dialogBusy = true;
			try {
				await uninstallPlugin(fetch, id);
				plugins = plugins.filter((p) => p.id !== id);
			} catch (e) {
				dialogError = message(e);
				return;
			} finally {
				dialogBusy = false;
			}
		}
		dialog = null;
	}

	async function toggle(p: Plugin, libraryId: string, enabled: boolean) {
		rowError = Object.fromEntries(Object.entries(rowError).filter(([id]) => id !== p.id));
		try {
			replace(await setPluginEnabled(fetch, p.id, libraryId, enabled));
		} catch (e) {
			rowError = { ...rowError, [p.id]: message(e) };
			plugins = [...plugins];
		}
	}

	async function remove(p: Plugin) {
		try {
			await uninstallPlugin(fetch, p.id);
			plugins = plugins.filter((x) => x.id !== p.id);
		} catch (e) {
			rowError = { ...rowError, [p.id]: message(e) };
		} finally {
			confirming = null;
		}
	}

	/** Every requested permission, with whether it is granted, per library where that applies. */
	function chips(p: Plugin) {
		return p.permissions.flatMap((r) => {
			const title = PERMISSION_LABELS[r.permission].title;
			if (!isLibraryPermission(r.permission)) {
				return [{ key: r.permission, label: title, granted: p.granted.includes(r.permission) }];
			}
			return p.libraries.map((l) => ({
				key: `${l.libraryId}:${r.permission}`,
				label: p.libraries.length > 1 ? `${title} (${libraryName(l.libraryId)})` : title,
				granted: l.granted.includes(r.permission)
			}));
		});
	}

	function needs(list: Plugin['libraries'][number]['missingRequired']): string {
		return list.map((x) => PERMISSION_LABELS[x].title).join(' and ');
	}
</script>

<svelte:head><title>Plugins · Jewelcase</title></svelte:head>

<PageHeader
	title="Plugins"
	subtitle="Add features to Jewelcase. Each plugin gets only the permissions you approve."
/>

<section class="add" aria-labelledby="add-title">
	<h2 id="add-title">Add a plugin</h2>
	<label
		class="drop"
		class:dragging
		ondragover={(e) => {
			e.preventDefault();
			dragging = true;
		}}
		ondragleave={() => (dragging = false)}
		ondrop={(e) => {
			e.preventDefault();
			dragging = false;
			pickFile(e.dataTransfer?.files);
		}}
	>
		<Icon name="upload" size={22} />
		<span><strong>Choose a plugin file</strong> or drop it here</span>
		<span class="muted small">A <code>.wasm</code> file</span>
		<input
			bind:this={fileInput}
			type="file"
			accept=".wasm,application/wasm"
			class="visually-hidden"
			disabled={installing}
			onchange={(e) => pickFile(e.currentTarget.files)}
		/>
	</label>
	<form
		class="url"
		onsubmit={(e) => {
			e.preventDefault();
			if (url.trim()) void install(() => installPluginUrl(fetch, url.trim()));
		}}
	>
		<input
			type="url"
			bind:value={url}
			placeholder="Or paste a link to one"
			aria-label="Plugin URL"
			disabled={installing}
		/>
		<button type="submit" disabled={installing || !url.trim()}>
			{installing ? 'Installing…' : 'Install'}
		</button>
	</form>
	{#if installError}<p class="error" role="alert">{installError}</p>{/if}
</section>

<section aria-labelledby="installed-title">
	<h2 id="installed-title">Installed</h2>
	{#if plugins.length === 0}
		<p class="empty muted">No plugins installed.</p>
	{:else}
		<ul class="plugins">
			{#each plugins as p (p.id)}
				<li class="card">
					<div class="top">
						<span class="icon"><Icon name="plugin" size={20} /></span>
						<div class="meta">
							<h3>{p.name}</h3>
							<p class="muted small">
								Version {p.version}{#if p.author}&nbsp;· by {p.author}{/if}
							</p>
						</div>
					</div>
					{#if p.description}<p class="description">{p.description}</p>{/if}

					{#each p.libraries as lib (lib.libraryId)}
						<label class="switch" class:off={lib.missingRequired.length > 0}>
							<input
								type="checkbox"
								role="switch"
								checked={lib.enabled}
								disabled={lib.missingRequired.length > 0}
								onchange={(e) => toggle(p, lib.libraryId, e.currentTarget.checked)}
							/>
							Enabled in {libraryName(lib.libraryId)}
						</label>
						{#if lib.missingRequired.length}
							<p class="muted small note">
								Needs {needs(lib.missingRequired)}.
								{#if lib.disabledReason}{lib.disabledReason}{/if}
							</p>
						{/if}
					{/each}

					{#if p.permissions.length}
						<ul class="chips" aria-label="Permissions">
							{#each chips(p) as c (c.key)}
								<li class:granted={c.granted}>
									<Icon name={c.granted ? 'check' : 'close'} size={12} />
									{c.label}
									<span class="visually-hidden">{c.granted ? 'granted' : 'not granted'}</span>
								</li>
							{/each}
						</ul>
					{/if}

					{#if rowError[p.id]}<p class="error" role="alert">{rowError[p.id]}</p>{/if}

					<div class="actions">
						<button
							type="button"
							onclick={() => {
								dialogError = null;
								dialog = { plugin: p, mode: 'edit' };
							}}
						>
							Permissions…
						</button>
						{#if confirming === p.id}
							<span class="confirm">
								Uninstall {p.name}?
								<button type="button" class="danger" onclick={() => remove(p)}>Uninstall</button>
								<button type="button" onclick={() => (confirming = null)}>Keep</button>
							</span>
						{:else}
							<button type="button" onclick={() => (confirming = p.id)}>Uninstall</button>
						{/if}
					</div>
				</li>
			{/each}
		</ul>
	{/if}
</section>

{#if dialog}
	<PermissionDialog
		plugin={dialog.plugin}
		mode={dialog.mode}
		{libraries}
		busy={dialogBusy}
		error={dialogError}
		onapprove={approve}
		oncancel={cancel}
	/>
{/if}

<style>
	section {
		max-width: 760px;
		margin-bottom: var(--space-6);
	}

	h2 {
		font-size: 18px;
		margin-bottom: var(--space-3);
	}

	.small {
		font-size: 13px;
	}

	.drop {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-1);
		padding: var(--space-5);
		border: 2px dashed var(--border);
		border-radius: var(--radius-l);
		color: var(--text-muted);
		text-align: center;
		cursor: pointer;
	}

	.drop strong {
		color: var(--accent);
	}

	.drop:hover,
	.drop.dragging,
	.drop:focus-within {
		border-color: var(--accent);
		background: var(--accent-soft);
	}

	.url {
		display: flex;
		gap: var(--space-2);
		margin-top: var(--space-3);
	}

	.url input {
		flex: 1;
		min-width: 0;
		height: 36px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font: inherit;
	}

	button {
		height: 34px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 17px;
		background: var(--surface-2);
		font-weight: 600;
		cursor: pointer;
	}

	button:hover:not(:disabled) {
		background: var(--surface-hover);
	}

	.url button {
		background: var(--accent);
		color: var(--accent-text);
	}

	button:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.danger {
		background: #dc2626;
		color: #fff;
	}

	.danger:hover:not(:disabled) {
		background: #b91c1c;
	}

	.plugins {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.card {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
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
		width: 40px;
		height: 40px;
		border-radius: var(--radius-m);
		background: var(--accent-soft);
		color: var(--accent);
	}

	h3 {
		font-size: 16px;
	}

	.meta p,
	.description,
	.note {
		margin: 0;
	}

	.switch {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-weight: 600;
		cursor: pointer;
	}

	.switch.off {
		cursor: default;
		color: var(--text-muted);
	}

	.switch input {
		width: 18px;
		height: 18px;
		accent-color: var(--accent);
	}

	.chips {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.chips li {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 2px 10px;
		border-radius: 12px;
		background: var(--surface-2);
		color: var(--text-muted);
		font-size: 12px;
		text-decoration: line-through;
	}

	.chips li.granted {
		background: var(--accent-soft);
		color: var(--accent);
		text-decoration: none;
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
	}

	.confirm {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
	}

	.error {
		margin: var(--space-2) 0 0;
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
