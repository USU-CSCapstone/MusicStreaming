<script lang="ts">
	import { isRedirect } from '@sveltejs/kit';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		disconnectMyPlugin,
		getMyPluginSettings,
		logout,
		setMyPluginSettings,
		setMySearchSharing
	} from '$lib/api/client';
	import type { PersonalPlugin } from '$lib/api/plugins';
	import type { User } from '$lib/api/types';
	import DeviceList from '$lib/components/DeviceList.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import SettingsDialog from '$lib/components/SettingsDialog.svelte';
	import { prefs } from '$lib/prefs.svelte';
	import { realtime } from '$lib/realtime.svelte';
	import { tagTypeName } from '$lib/tags';

	let { data } = $props();

	const me = $derived(page.data.me);
	// Svelte 5 lets a derived be overwritten locally; it resets when the page reloads its data.
	let connectable = $derived(data.connectable);
	let connecting = $state<PersonalPlugin | null>(null);
	let connectionError = $state('');
	const roles: Record<User['role'], string> = { owner: 'Owner', admin: 'Admin', user: 'User' };
	const tagTypes = $derived(page.data.tagTypes ?? []);

	let error = $state('');
	let busy = $state(false);

	/** Ends the session and loads the login page; on failure shows the error and keeps the realtime connection running. */
	async function logOut() {
		busy = true;
		error = '';
		// Ending the session closes the socket; left running, the client would take that for a
		// sign-out elsewhere and race this page to the login page.
		realtime.stop();
		try {
			await logout(fetch);
		} catch (e) {
			// Already logged out elsewhere is as good as logging out now.
			if (!isRedirect(e)) {
				error = e instanceof Error ? e.message : String(e);
				busy = false;
				realtime.start();
				return;
			}
		}
		// A fresh start, so nothing of this session, playback above all, outlives it.
		location.assign(resolve('/login'));
	}

	function update(id: string, change: Partial<PersonalPlugin>) {
		connectable = connectable.map((p) => (p.id === id ? { ...p, ...change } : p));
	}

	async function disconnect(p: PersonalPlugin) {
		connectionError = '';
		try {
			await disconnectMyPlugin(fetch, p.id);
			// Disconnecting stops sharing searches too.
			update(p.id, { connected: false, sharesSearches: false });
		} catch (e) {
			connectionError = e instanceof Error ? e.message : String(e);
		}
	}

	// Applied at once and confirmed in the background, put back if refused
	// (`requirements/general.md` §3.4).
	async function shareSearches(p: PersonalPlugin, sharing: boolean) {
		connectionError = '';
		update(p.id, { sharesSearches: sharing });
		try {
			await setMySearchSharing(fetch, p.id, sharing);
		} catch (e) {
			update(p.id, { sharesSearches: !sharing });
			connectionError = e instanceof Error ? e.message : String(e);
		}
	}
</script>

<svelte:head><title>Account · Jewelcase</title></svelte:head>

<PageHeader title="Account" />
{#if me}
	<section>
		<p>
			<strong>{me.displayName}</strong>
			<span class="muted">{me.username} · {roles[me.role]}</span>
		</p>
		<!-- Told in the app, not only in documentation (`requirements/users.md` §8). -->
		<p class="muted">
			The admins of this server can see your listening data: your playlists, queues, downloads,
			history, and statistics. They can't sign in as you or change any of it.
		</p>
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<button type="button" onclick={logOut} disabled={busy}>
			{busy ? 'Logging out…' : 'Log out'}
		</button>
	</section>
{/if}

{#if me && me.role !== 'user'}
	<!-- On a phone there is no sidebar, so the admin pages are reached from here. -->
	<section aria-labelledby="administration">
		<h2 id="administration">Administration</h2>
		<ul class="admin">
			<li>
				<a href={resolve('/admin/libraries')}>Libraries</a>
				<span class="muted">Scan your music, and see how the last scan went.</span>
			</li>
			<li>
				<a href={resolve('/admin/plugins')}>Plugins</a>
				<span class="muted">Add features, and approve what each one reaches.</span>
			</li>
		</ul>
	</section>
{/if}

{#if me}
	<section aria-labelledby="devices">
		<h2 id="devices">Devices</h2>
		<p class="muted">
			Everywhere you're signed in. Music plays on one at a time, and any of the others can control
			it or take it over.
		</p>
		<DeviceList />
	</section>
{/if}

{#if me}
	<section aria-labelledby="columns">
		<h2 id="columns">Song columns</h2>
		<p class="muted">
			Which tags show as columns in lists of songs, where each column's heading filters by them.
			Tags come from your music files' own fields, such as genre and mood.
		</p>
		{#if tagTypes.length}
			<ul>
				{#each tagTypes as type (type.key)}
					<li>
						<label class="column">
							<input
								type="checkbox"
								checked={prefs.showsTagColumn(type.key)}
								onchange={(e) => prefs.showTagColumn(type.key, e.currentTarget.checked)}
							/>
							{tagTypeName(type.key, tagTypes)}
						</label>
					</li>
				{/each}
			</ul>
		{:else}
			<p class="muted">Your music carries no genres or moods yet.</p>
		{/if}
	</section>
{/if}

{#if me && page.data.library}
	<section aria-labelledby="hidden">
		<h2 id="hidden">Hidden songs</h2>
		<p class="muted">
			Songs you hid are gone from your library: from every list, search, and anything played for
			you. Hide a song from its ⋯ menu, and bring it back from Hidden Songs.
		</p>
		<a class="button" href={resolve('/account/hidden')}>
			<Icon name="hide" size={16} />
			Hidden Songs{#if data.hiddenCount !== null}<span class="muted">
					· {data.hiddenCount.toLocaleString()}</span
				>{/if}
		</a>
	</section>
{/if}

{#if connectable.length}
	<section aria-labelledby="connections">
		<h2 id="connections">Connections</h2>
		<!-- Told where the user shares, not after they clear (`requirements/users.md` §7). -->
		<p class="muted">
			Plugins that act for you with your own account, such as sending what you play to a scrobbling
			service. Each sees your listening only once you connect it. What a plugin sends to its service
			stays there: clearing your history here asks it to forget, but can't take it back.
		</p>
		{#if connectionError}<p class="error" role="alert">{connectionError}</p>{/if}
		<ul>
			{#each connectable as p (p.id)}
				<li>
					<div>
						<strong>{p.name}</strong>
						<span class="muted">{p.connected ? 'Connected' : (p.description ?? '')}</span>
						{#if p.hearsLikes}
							<span class="muted hears">
								Gets the songs you like and hide, starting with those you already have.
							</span>
						{/if}
					</div>
					<button type="button" onclick={() => (connecting = p)}>
						{p.connected ? 'Change…' : 'Connect…'}
					</button>
					{#if p.connected}
						<button type="button" onclick={() => disconnect(p)}>Disconnect</button>
					{/if}
					{#if p.connected && p.asksForSearches}
						<label class="share">
							<input
								type="checkbox"
								checked={p.sharesSearches}
								onchange={(e) => shareSearches(p, e.currentTarget.checked)}
							/>
							<span>
								Share my searches with {p.name}
								<span class="muted">
									It gets each search you settle on, including ones that found nothing, and may keep
									them or send them on. Removing a search asks it to forget, but can't take back
									what it already sent.
								</span>
							</span>
						</label>
					{/if}
				</li>
			{/each}
		</ul>
	</section>
{/if}

{#if connecting}
	{@const p = connecting}
	<SettingsDialog
		title="Connect {p.name}"
		load={() => getMyPluginSettings(fetch, p.id)}
		save={async (values) => {
			await setMyPluginSettings(fetch, p.id, values);
			update(p.id, { connected: true });
		}}
		onclose={() => (connecting = null)}
	/>
{/if}

<style>
	section {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-3);
		max-width: 560px;
	}

	section + section {
		margin-top: var(--space-6);
	}

	h2 {
		font-size: 18px;
	}

	ul {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: 100%;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
	}

	.admin a {
		color: var(--accent);
		font-weight: 600;
	}

	.column {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		cursor: pointer;
	}

	.share {
		display: flex;
		flex-basis: 100%;
		align-items: flex-start;
		gap: var(--space-2);
		cursor: pointer;
	}

	.share input {
		/* Lined up with the first line of the label, and a comfortable target for touch. */
		width: 18px;
		height: 18px;
		margin: 2px 0 0;
		accent-color: var(--accent);
	}

	.share .muted {
		display: block;
		font-size: 13px;
	}

	li div {
		flex: 1;
		min-width: 0;
	}

	.hears {
		display: block;
	}

	p {
		margin: 0;
	}

	strong {
		display: block;
		font-size: 16px;
	}

	.error {
		color: var(--danger);
	}

	button {
		height: 36px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		font-weight: 500;
		cursor: pointer;
	}

	button:hover,
	.button:hover {
		background: var(--surface-hover);
	}

	/* A link that looks like the page's buttons. */
	.button {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		height: 36px;
		padding: 0 var(--space-4);
		border: 1px solid var(--border);
		border-radius: var(--radius-m);
		background: var(--surface);
		color: var(--text);
		font-weight: 500;
		text-decoration: none;
	}

	button:disabled {
		opacity: 0.6;
		cursor: default;
	}
</style>
