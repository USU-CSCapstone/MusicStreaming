<script lang="ts">
	// A plugin's settings, as its manifest declares them (`requirements/plugins.md` §6): an
	// admin's, for every library or one library's own, or a user's own. Secrets are never shown
	// again: left empty, a saved one is kept, and Clear removes it.
	import { onMount } from 'svelte';
	import type { PluginSettings, SettingSchema } from '$lib/api/plugins';

	let {
		title,
		libraries = [],
		load: fetchSettings,
		save: saveSettings,
		onclose
	}: {
		title: string;
		/** Libraries with settings of their own, chosen between; none for a user's own. */
		libraries?: { id: string; name: string }[];
		/** The settings at a level: a library's ID, or undefined for every library. */
		load: (libraryId?: string) => Promise<PluginSettings>;
		save: (values: Record<string, unknown>, libraryId?: string) => Promise<unknown>;
		onclose: () => void;
	} = $props();

	let dialog: HTMLDialogElement;
	const uid = $props.id();
	const titleId = `settings-${uid}`;

	/** The level being edited: '' for every library, else a library's ID. */
	let level = $state('');
	let loaded = $state<PluginSettings | null>(null);
	/** What is in the form, as text for text and numbers. */
	let draft = $state<Record<string, string | boolean>>({});
	let cleared = $state<string[]>([]);
	let error = $state('');
	let busy = $state(false);

	type Schema = { properties?: Record<string, SettingSchema>; required?: string[] };
	const schema = $derived(loaded?.schema as Schema | undefined);
	const fields = $derived(Object.entries(schema?.properties ?? {}));
	const required = $derived(schema?.required ?? []);

	async function load() {
		error = '';
		try {
			loaded = await fetchSettings(level || undefined);
			const values = loaded.values as Record<string, string | number | boolean>;
			draft = Object.fromEntries(
				Object.entries(values).map(([k, v]) => [k, typeof v === 'boolean' ? v : String(v)])
			);
			cleared = [];
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		}
	}

	onMount(() => {
		dialog.showModal();
		void load();
	});

	/** What to send: each field filled in, a cleared secret as null, and the rest left out. */
	function values(): Record<string, unknown> {
		const out: Record<string, unknown> = {};
		for (const [name, s] of fields) {
			if (cleared.includes(name)) {
				out[name] = null;
				continue;
			}
			const v = draft[name];
			if (v === undefined || v === '') continue;
			out[name] = s.type === 'number' || s.type === 'integer' ? Number(v) : v;
		}
		return out;
	}

	async function save(e: SubmitEvent) {
		e.preventDefault();
		busy = true;
		error = '';
		try {
			await saveSettings(values(), level || undefined);
			onclose();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	bind:this={dialog}
	aria-labelledby={titleId}
	oncancel={(e) => {
		e.preventDefault();
		if (!busy) onclose();
	}}
>
	<form onsubmit={save}>
		<h2 id={titleId}>{title}</h2>

		{#if libraries.length}
			<label>
				For
				<select bind:value={level} onchange={() => void load()} disabled={busy}>
					<option value="">Every library</option>
					{#each libraries as lib (lib.id)}<option value={lib.id}>{lib.name} only</option>{/each}
				</select>
			</label>
			{#if level}
				<p class="muted small">Anything left empty here uses the setting for every library.</p>
			{/if}
		{/if}

		{#each fields as [name, s] (name)}
			{@const label = s.title ?? name}
			{@const isSet = loaded?.secretsSet.includes(name) && !cleared.includes(name)}
			{#if s.type === 'boolean'}
				<label class="check">
					<input
						type="checkbox"
						checked={(draft[name] ?? s.default ?? false) === true}
						onchange={(e) => (draft[name] = e.currentTarget.checked)}
					/>
					{label}
				</label>
			{:else}
				<label>
					<span
						>{label}{#if required.includes(name)}<span class="muted"> (required)</span>{/if}</span
					>
					{#if s.enum}
						<select bind:value={draft[name]}>
							<option value=""
								>{s.default !== undefined ? `Default (${s.default})` : 'Not set'}</option
							>
							{#each s.enum as choice (choice)}<option value={String(choice)}>{choice}</option
								>{/each}
						</select>
					{:else}
						<span class="input">
							<input
								type={s.writeOnly ? 'password' : s.type === 'string' ? 'text' : 'number'}
								step={s.type === 'integer' ? 1 : 'any'}
								bind:value={draft[name]}
								placeholder={isSet
									? 'Saved; type to replace it'
									: s.default !== undefined
										? String(s.default)
										: ''}
								autocomplete={s.writeOnly ? 'off' : undefined}
							/>
							{#if s.writeOnly && isSet}
								<button type="button" onclick={() => (cleared = [...cleared, name])}>Clear</button>
							{/if}
						</span>
					{/if}
				</label>
			{/if}
			{#if s.description}<p class="muted small">{s.description}</p>{/if}
		{/each}

		{#if error}<p class="error" role="alert">{error}</p>{/if}

		<footer>
			<button type="button" onclick={onclose} disabled={busy}>Cancel</button>
			<button type="submit" class="primary" disabled={busy || !loaded}
				>{busy ? 'Checking…' : 'Save'}</button
			>
		</footer>
	</form>
</dialog>

<style>
	dialog {
		width: min(440px, calc(100vw - 2 * var(--space-4)));
		padding: 0;
		border: 0;
		border-radius: var(--radius-l);
		background: var(--surface);
		color: var(--text);
		box-shadow: var(--shadow);
	}

	dialog::backdrop {
		background: #0006;
	}

	form {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
		padding: var(--space-5);
	}

	h2 {
		font-size: 18px;
	}

	p {
		margin: 0;
	}

	.small {
		font-size: 12px;
	}

	label {
		display: flex;
		flex-direction: column;
		gap: var(--space-1);
		font-weight: 500;
	}

	label.check {
		flex-direction: row;
		align-items: center;
		gap: var(--space-2);
	}

	.input {
		display: flex;
		gap: var(--space-2);
	}

	input:not([type='checkbox']),
	select {
		flex: 1;
		height: 36px;
		padding: 0 var(--space-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface-2);
		color: var(--text);
		font: inherit;
		font-weight: 400;
	}

	.error {
		color: var(--danger);
	}

	footer {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-2);
		margin-top: var(--space-2);
	}

	button {
		height: 36px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font: inherit;
		font-weight: 500;
		cursor: pointer;
	}

	button.primary {
		border-color: var(--accent);
		background: var(--accent);
		color: var(--accent-text);
	}

	button:disabled {
		opacity: 0.6;
		cursor: default;
	}
</style>
