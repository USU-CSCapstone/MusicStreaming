<script lang="ts" module>
	import type { PermissionGrants } from '$lib/api/plugins';

	/** What the admin approved: the grants, and which libraries to enable it in. */
	export type Decision = { grants: PermissionGrants; enable: Record<string, boolean> };
</script>

<script lang="ts">
	import {
		PERMISSION_LABELS,
		isLibraryPermission,
		type PermissionName,
		type PermissionRequest,
		type Plugin
	} from '$lib/api/plugins';
	import Icon from './Icon.svelte';

	let {
		plugin,
		libraries,
		mode,
		busy = false,
		error = null,
		onapprove,
		oncancel
	}: {
		plugin: Plugin;
		libraries: { id: string; name: string }[];
		/** `install` right after adding it, where cancelling removes it again. */
		mode: 'install' | 'edit';
		busy?: boolean;
		error?: string | null;
		onapprove: (decision: Decision) => void;
		oncancel: () => void;
	} = $props();

	let dialog: HTMLDialogElement;
	const uid = $props.id();
	const titleId = `permissions-${uid}`;

	const pluginWide = $derived(plugin.permissions.filter((r) => !isLibraryPermission(r.permission)));
	const perLibrary = $derived(plugin.permissions.filter((r) => isLibraryPermission(r.permission)));
	const key = (p: PermissionName, libraryId?: string) => (libraryId ? `${libraryId}:${p}` : p);

	// Nothing is pre-approved (`requirements/plugins.md` §4.2): only what was already granted,
	// including grants kept from an earlier install, starts checked.
	let checked = $state<Record<string, boolean>>({});
	let enable = $state<Record<string, boolean>>({});
	$effect.pre(() => {
		const next: Record<string, boolean> = {};
		for (const p of plugin.granted) next[key(p)] = true;
		for (const lib of plugin.libraries)
			for (const p of lib.granted) next[key(p, lib.libraryId)] = true;
		checked = next;
		enable = Object.fromEntries(
			libraries.map((l) => [
				l.id,
				mode === 'install' || !!plugin.libraries.find((x) => x.libraryId === l.id)?.enabled
			])
		);
	});

	/** Required permissions still unchecked for a library, plugin-wide ones included. */
	function missing(libraryId: string): PermissionName[] {
		return plugin.permissions
			.filter((r) => r.required)
			.filter(
				(r) =>
					!checked[key(r.permission, isLibraryPermission(r.permission) ? libraryId : undefined)]
			)
			.map((r) => r.permission);
	}

	function decision(): Decision {
		return {
			grants: {
				granted: pluginWide.filter((r) => checked[key(r.permission)]).map((r) => r.permission),
				libraries: libraries.map((l) => ({
					libraryId: l.id,
					granted: perLibrary
						.filter((r) => checked[key(r.permission, l.id)])
						.map((r) => r.permission)
				}))
			},
			enable: Object.fromEntries(
				libraries.map((l) => [l.id, enable[l.id] && missing(l.id).length === 0])
			)
		};
	}

	function approveAll() {
		for (const r of pluginWide) checked[key(r.permission)] = true;
		for (const l of libraries) {
			for (const r of perLibrary) checked[key(r.permission, l.id)] = true;
			enable[l.id] = true;
		}
		onapprove(decision());
	}

	function names(list: PermissionName[]): string {
		return list.map((p) => PERMISSION_LABELS[p].title).join(' and ');
	}

	/** Grants restored from an earlier install, which start checked (`requirements/plugins.md` §5). */
	const restored = $derived(
		mode === 'install' &&
			(plugin.granted.length > 0 || plugin.libraries.some((l) => l.granted.length > 0))
	);

	$effect(() => {
		dialog.showModal();
		return () => dialog.close();
	});
</script>

{#snippet row(req: PermissionRequest, libraryId?: string)}
	{@const label = PERMISSION_LABELS[req.permission]}
	<label class="perm">
		<input type="checkbox" bind:checked={checked[key(req.permission, libraryId)]} disabled={busy} />
		<span class="body">
			<span class="title">
				{label.title}
				{#if req.required}<span class="badge">Required</span>{/if}
			</span>
			<span class="reason">“{req.reason}”</span>
			{#if req.permission === 'network'}
				<span class="detail">
					{req.destinations?.includes('*')
						? 'Any destination'
						: `Only ${req.destinations?.join(', ')}`}
				</span>
			{:else if req.permission === 'libraryAdd' || req.permission === 'libraryChange'}
				<!-- The risk is stated where the decision is made (`requirements/plugins.md` §4.2). -->
				<span class="warn"><Icon name="warning" size={14} /> {label.detail}</span>
			{:else}
				<span class="detail">{label.detail}</span>
			{/if}
		</span>
	</label>
{/snippet}

<dialog
	bind:this={dialog}
	aria-labelledby={titleId}
	oncancel={(e) => {
		e.preventDefault();
		if (!busy) oncancel();
	}}
>
	<form
		onsubmit={(e) => {
			e.preventDefault();
			onapprove(decision());
		}}
	>
		<header>
			<span class="icon"><Icon name="plugin" size={22} /></span>
			<div>
				<h2 id={titleId}>{mode === 'install' ? 'Allow' : 'Permissions for'} {plugin.name}</h2>
				<p class="muted">
					Version {plugin.version}{#if plugin.author}&nbsp;· by {plugin.author}{/if}
				</p>
			</div>
		</header>

		{#if plugin.description}<p class="description">{plugin.description}</p>{/if}

		{#if restored}
			<p class="note">Permissions you approved when it was last installed are kept.</p>
		{/if}

		{#if plugin.permissions.length === 0}
			<p class="muted">This plugin asks for no permissions.</p>
		{/if}

		{#if pluginWide.length}
			<fieldset>
				<legend>Across this server</legend>
				{#each pluginWide as req (req.permission)}{@render row(req)}{/each}
			</fieldset>
		{/if}

		{#each libraries as lib (lib.id)}
			{@const needs = missing(lib.id)}
			<fieldset>
				<legend>In “{lib.name}”</legend>
				{#each perLibrary as req (req.permission)}{@render row(req, lib.id)}{/each}
				<label class="enable">
					<input
						type="checkbox"
						checked={enable[lib.id] && needs.length === 0}
						disabled={busy || needs.length > 0}
						onchange={(e) => (enable[lib.id] = e.currentTarget.checked)}
					/>
					Enable in {lib.name}
				</label>
				{#if needs.length}
					<p class="needs">Needs {names(needs)} before it can be enabled.</p>
				{/if}
			</fieldset>
		{:else}
			{#if perLibrary.length}
				<p class="muted">There is no library to grant library permissions in yet.</p>
			{/if}
		{/each}

		{#if error}<p class="error" role="alert">{error}</p>{/if}

		<footer>
			<button type="button" class="secondary" disabled={busy} onclick={oncancel}>
				{mode === 'install' ? 'Cancel install' : 'Cancel'}
			</button>
			<span class="spacer"></span>
			<button type="submit" class="secondary" disabled={busy}>Approve selected</button>
			<button type="button" class="primary" disabled={busy} onclick={approveAll}>Approve all</button
			>
		</footer>
	</form>
</dialog>

<style>
	dialog {
		width: min(520px, calc(100vw - 32px));
		max-height: calc(100dvh - 32px);
		padding: 0;
		border: 1px solid var(--border);
		border-radius: var(--radius-l);
		background: var(--surface);
		color: var(--text);
		box-shadow: 0 20px 60px #00000040;
	}

	dialog::backdrop {
		background: #00000066;
	}

	form {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding: var(--space-5);
	}

	header {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.icon {
		display: grid;
		place-items: center;
		width: 44px;
		height: 44px;
		border-radius: var(--radius-m);
		background: var(--accent-soft);
		color: var(--accent);
		flex-shrink: 0;
	}

	h2 {
		font-size: 18px;
	}

	header p,
	.description {
		margin: 0;
	}

	fieldset {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		margin: 0;
		padding: 0;
		border: 0;
	}

	legend {
		padding: 0 0 var(--space-2);
		font-size: 12px;
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		color: var(--text-muted);
	}

	.perm,
	.enable {
		display: flex;
		align-items: flex-start;
		gap: var(--space-3);
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-m);
		background: var(--surface-2);
		cursor: pointer;
	}

	.enable {
		align-items: center;
		background: transparent;
		font-weight: 600;
	}

	input[type='checkbox'] {
		width: 18px;
		height: 18px;
		margin: 2px 0 0;
		accent-color: var(--accent);
		flex-shrink: 0;
	}

	.enable input {
		margin: 0;
	}

	.body {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.title {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-weight: 600;
	}

	.badge {
		padding: 0 6px;
		border-radius: 4px;
		background: var(--accent-soft);
		color: var(--accent);
		font-size: 11px;
		font-weight: 600;
		line-height: 18px;
	}

	.reason {
		font-style: italic;
	}

	.detail {
		color: var(--text-muted);
		font-size: 13px;
	}

	.warn {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		color: #c2410c;
		font-size: 13px;
	}

	.note {
		margin: 0;
		padding: var(--space-2) var(--space-3);
		border-radius: var(--radius-m);
		background: var(--accent-soft);
		font-size: 13px;
	}

	.needs {
		margin: 0;
		padding: 0 var(--space-3);
		color: var(--text-muted);
		font-size: 13px;
	}

	.error {
		margin: 0;
		color: #dc2626;
	}

	footer {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.spacer {
		flex: 1;
	}

	footer button {
		height: 36px;
		padding: 0 var(--space-4);
		border: 0;
		border-radius: 18px;
		font-weight: 600;
		cursor: pointer;
	}

	.secondary {
		background: var(--surface-2);
	}

	.secondary:hover:not(:disabled) {
		background: var(--surface-hover);
	}

	.primary {
		background: var(--accent);
		color: var(--accent-text);
	}

	button:disabled {
		opacity: 0.5;
		cursor: default;
	}

	@media (prefers-color-scheme: dark) {
		.warn {
			color: #fb923c;
		}

		.error {
			color: #f87171;
		}
	}
</style>
