<script lang="ts">
	import { onMount } from 'svelte';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { disconnectMyPlugin, getMyPluginSettings, setMyPluginSettings } from '$lib/api/client';
	import type { PersonalPlugin } from '$lib/api/plugins';
	import type { User } from '$lib/api/types';
	import DeviceList from '$lib/components/DeviceList.svelte';
	import Icon from '$lib/components/Icon.svelte';
	import PageHeader from '$lib/components/PageHeader.svelte';
	import SettingsDialog from '$lib/components/SettingsDialog.svelte';
	import { catalog } from '$lib/catalog/client.svelte';
	import { connectivity } from '$lib/connectivity.svelte';
	import { imageCacheSize } from '$lib/image-cache';
	import { lastKnown, remember, setLogoutPending } from '$lib/last-known';
	import { finishLogout } from '$lib/logout';
	import { prefs } from '$lib/prefs.svelte';
	import { queue } from '$lib/queue/client.svelte';
	import { realtime } from '$lib/realtime.svelte';
	import { definition } from '$lib/sync/mutations';
	import { QUEUE_SCOPE } from '$lib/sync/mutations/queue';
	import { type SetAside, sender } from '$lib/sync/sender';
	import { tagTypeName } from '$lib/tags';

	let { data } = $props();

	const me = $derived(page.data.me);
	// Svelte 5 lets a derived be overwritten locally; it resets when the page reloads its data.
	let connectable = $derived(data.connectable ?? []);
	let connecting = $state<PersonalPlugin | null>(null);
	let connectionError = $state('');
	const roles: Record<User['role'], string> = { owner: 'Owner', admin: 'Admin', user: 'User' };
	const tagTypes = $derived(page.data.tagTypes ?? []);

	/** What the device keeps of the art seen online, once counted. */
	let images = $state<{ count: number; bytes: number } | null>(null);
	/** Formats a size in megabytes for the storage sections. */
	const megabytes = new Intl.NumberFormat(undefined, {
		style: 'unit',
		unit: 'megabyte',
		maximumFractionDigits: 1
	});
	/** The count and size of the kept images, as one line of text. */
	const imageSummary = ({ count, bytes }: { count: number; bytes: number }) =>
		`${count} ${count === 1 ? 'image' : 'images'} · ${megabytes.format(bytes / 1_000_000)}`;
	/** Whether the browser promised to keep this device's storage: unknown where it cannot say. */
	let persisted = $state<boolean | null>(null);
	onMount(() => {
		if (me) void imageCacheSize(me.id).then((size) => (images = size));
		navigator.storage?.persisted?.().then(
			(kept) => (persisted = kept),
			() => {}
		);
	});
	/** The libraries the device holds or could not take, named as the account last saw them. */
	const held = $derived(
		Object.entries(catalog.libraries).flatMap(([id, state]) =>
			state.onDevice || state.noRoom
				? [
						{
							id,
							name:
								lastKnown().libraries?.items.find((library) => library.id === id)?.name ??
								'A library',
							...state
						}
					]
				: []
		)
	);

	/** What the log set aside after it kept failing, for the user to retry or discard. */
	let setAside = $state<SetAside[]>([]);
	/**
	 * Each set-aside change with those set aside because they were made on it, which go with it; one
	 * whose own has gone stands alone.
	 */
	const asideGroups = $derived(
		setAside
			.filter((rec) => !rec.with || !setAside.some((other) => other.key === rec.with))
			.map((rec) => ({ rec, more: setAside.filter((other) => other.with === rec.key).length }))
	);
	/** Reads again what the log set aside. */
	const readSetAside = async () => (setAside = await sender.setAside());
	$effect(() => {
		if (!me) return;
		void readSetAside();
		// One set aside while the page is open shows here at once.
		return sender.onSettled((_rec, _outcome, { dead }) => dead && void readSetAside());
	});

	/** What the set-aside `rec` was, and how many were made on it, in a few words. */
	function asideWhat(rec: SetAside, more: number): string {
		const what = definition(rec.name)?.what(rec.args) ?? 'a change from a newer version of the app';
		const extra = more ? ` (and ${more} ${more === 1 ? 'change' : 'changes'} made on it)` : '';
		return what.charAt(0).toUpperCase() + what.slice(1) + extra;
	}

	/** Puts the set-aside change `key` back to be sent, or lets it go, then reads the list again. */
	async function resolveAside(key: string, how: 'retry' | 'discard') {
		await (how === 'retry' ? sender.retry(key) : sender.discard(key));
		await readSetAside();
	}

	let busy = $state(false);
	/**
	 * How many plays and changes have not reached the server, once logging out found some: the user
	 * is asked before they are deleted. Those set aside are among them.
	 */
	let unsent = $state(0);
	/** How many of `unsent` were set aside. */
	let unsentAside = $state(0);

	/** How long the server is waited for at log out; a server out of reach is told later. */
	const LOGOUT_WAIT_MS = 3000;
	/** How long the queue is waited for to write what it holds, which is counted among the unsent. */
	const QUEUE_SAVE_WAIT_MS = 500;

	/**
	 * Logs out: what the log holds is sent if the server can be reached, and the user is asked
	 * before what is left is deleted with everything else. The device is wiped first, with a marker
	 * left for the server, which is told now if it answers in time and otherwise as soon as it can
	 * be reached (`$lib/logout`). Works with no server in reach, which a session cookie only the
	 * server can end needs.
	 */
	async function logOut(anyway = false) {
		busy = true;
		if (!anyway) {
			await Promise.race([sender.flush(), new Promise((done) => setTimeout(done, LOGOUT_WAIT_MS))]);
			// The queue's answer comes after every change its worker gave the log, so all are counted.
			await Promise.race([
				queue.save(),
				new Promise((done) => setTimeout(done, QUEUE_SAVE_WAIT_MS))
			]);
			unsentAside = (await sender.setAside()).length;
			unsent = (await sender.pending()) + unsentAside;
			if (unsent > 0) {
				busy = false;
				return;
			}
		}
		// Ending the session closes the socket; left running, the client would take that for a
		// sign-out elsewhere and race this page to the login page.
		realtime.stop();
		setLogoutPending(true);
		await catalog.signOut(true);
		await Promise.race([
			finishLogout(fetch),
			new Promise((done) => setTimeout(done, LOGOUT_WAIT_MS))
		]);
		// A fresh start, so nothing of this session, playback above all, outlives it.
		location.assign(resolve('/login'));
	}

	/** Changes plugin `id` as shown, and as kept for the page offline. */
	function update(id: string, change: Partial<PersonalPlugin>) {
		connectable = connectable.map((p) => (p.id === id ? { ...p, ...change } : p));
		remember({ plugins: connectable });
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

	/**
	 * Turns sharing searches with `p` on or off: applied at once and sent through the log, online or
	 * offline, and put back below if the server refuses it (`requirements/general.md` §3.4).
	 */
	function shareSearches(p: PersonalPlugin, sharing: boolean) {
		connectionError = '';
		update(p.id, { sharesSearches: sharing });
		sender.enqueue('shareSearches', { pluginId: p.id, name: p.name, sharing });
	}
	// Puts a switch back when the log drops its change.
	$effect(() =>
		sender.onSettled(({ name, args }, outcome) => {
			if (name !== 'shareSearches' || outcome !== 'dropped') return;
			const { pluginId, sharing } = args as { pluginId: string; sharing: boolean };
			update(pluginId, { sharesSearches: !sharing });
		})
	);
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
		{#if unsent > 0}
			<p role="alert">
				{unsent} plays and changes haven't reached the server{unsentAside
					? `, ${unsentAside} of them set aside`
					: ''}. Log out anyway?
			</p>
			<button type="button" onclick={() => logOut(true)} disabled={busy}>Log out anyway</button>
			<button type="button" onclick={() => (unsent = 0)} disabled={busy}>Cancel</button>
		{:else}
			<button type="button" onclick={() => logOut()} disabled={busy}>
				{busy ? 'Logging out…' : 'Log out'}
			</button>
		{/if}
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
	<section aria-labelledby="offline">
		<h2 id="offline">Offline</h2>
		<p class="muted">
			Browse what's on this device without using the network, to protect a metered connection.
			Nothing is sent to or fetched from the server until you turn this off, and then this device
			catches up. It applies to this device only.
		</p>
		<label class="column">
			<input
				type="checkbox"
				role="switch"
				checked={connectivity.workOffline}
				onchange={(e) => connectivity.setWorkOffline(e.currentTarget.checked)}
			/>
			Work offline
		</label>
	</section>
{/if}

{#if me && asideGroups.length}
	<section aria-labelledby="set-aside">
		<h2 id="set-aside">Changes not sent</h2>
		<p class="muted">
			These kept failing to reach the server, so they were set aside. Retry sends one again; Discard
			lets it go and takes it back on this device. A change to the queue can only be discarded.
		</p>
		<ul>
			{#each asideGroups as { rec, more } (rec.key)}
				<li>
					<div>
						<strong>{asideWhat(rec, more)}</strong>
						<span class="muted">
							Made {new Date(rec.createdAt).toLocaleString()}. {rec.why}
						</span>
					</div>
					<!-- Sent now, a queue change this old would make its queue the playing one everywhere. -->
					{#if rec.scope !== QUEUE_SCOPE}
						<button type="button" onclick={() => resolveAside(rec.key, 'retry')}>Retry</button>
					{/if}
					<button type="button" onclick={() => resolveAside(rec.key, 'discard')}>Discard</button>
				</li>
			{/each}
		</ul>
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

{#if me}
	<section aria-labelledby="stored-images">
		<h2 id="stored-images">Stored images</h2>
		<p class="muted">
			Art you've seen is kept on this device so it still shows offline. The oldest is dropped when
			there's too much, and all of it is deleted when you log out.
		</p>
		{#if images}
			<p>{imageSummary(images)}</p>
		{/if}
	</section>
{/if}

{#if me}
	<section aria-labelledby="storage">
		<h2 id="storage">Storage</h2>
		<p class="muted">
			Each library is copied to this device so it works offline. Jewelcase asks the browser not to
			clear that copy when the device runs low on space.
		</p>
		{#if persisted !== null}
			<p>
				{persisted
					? 'The browser has agreed to keep it.'
					: 'The browser has not agreed to keep it, and may clear it. It would then be downloaded again.'}
			</p>
		{/if}
		<ul>
			{#each held as library (library.id)}
				<li>
					<strong>{library.name}</strong>
					{#if library.noRoom}
						<span class="muted">
							{library.onDevice ? 'Not downloaded again' : 'Not downloaded'}: a
							{library.onDevice ? 'new copy' : 'copy'} needs
							{megabytes.format(library.noRoom.need / 1_000_000)} free and this device has
							{megabytes.format(library.noRoom.free / 1_000_000)}.
							{library.onDevice
								? 'It keeps up with the server’s changes where it can, and is tried again the next time the app starts or reconnects.'
								: 'It is tried again the next time the app starts or reconnects.'}
						</span>
					{:else}
						<span class="muted">On this device</span>
					{/if}
				</li>
			{/each}
		</ul>
	</section>
{/if}

{#if data.connectable === null}
	<section aria-labelledby="connections">
		<h2 id="connections">Connections</h2>
		<p class="muted">Unavailable offline. This needs the server, which this device can't reach.</p>
	</section>
{:else if connectable.length}
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
