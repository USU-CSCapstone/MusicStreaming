<script lang="ts">
	import type { Device } from '$lib/api/types';
	import { controls } from '$lib/controls.svelte';
	import { playback } from '$lib/playback.svelte';
	import { realtime } from '$lib/realtime.svelte';
	import Icon from './Icon.svelte';

	// The account's devices, one list wherever it is shown: from the player, and in account
	// settings (`requirements/realtime.md` §1, `requirements/users.md` §4). For each, what
	// matters for choosing it: its name, its type, whether it is connected, and what it plays.
	// One not connected is shown, not hidden, so one asleep is not hunted for. While this device
	// is offline, nothing is shown as current (§6). This device can take playback from elsewhere,
	// and any other connected one can be chosen to move it to (§3).

	/** This device first, then those connected, each as the server orders them: last seen first. */
	const rank = (device: Device) => (device.id === realtime.deviceId ? 0 : device.connected ? 1 : 2);
	const devices = $derived([...playback.devices].sort((a, b) => rank(a) - rank(b)));
	const stale = $derived(realtime.offline);
	/** The device holding playback, said aloud as playback moves, as the buttons here move it. */
	const holder = $derived(devices.find((device) => device.id === playback.state?.activeDeviceId));
	let list = $state<HTMLUListElement>();
	/**
	 * Whether there is playback elsewhere, playing or stopped, for this device to take: offline
	 * too, from where it was last heard of, and only of the library this device plays from.
	 */
	const takeable = $derived(controls.remote && controls.pickable);

	/** Whether playback can be moved to `device`: connected, not playing it, and with something to move. */
	function movable(device: Device): boolean {
		const state = playback.state;
		if (stale || !device.connected || device.id === realtime.deviceId) return false;
		return state?.trackId != null && device.id !== state.activeDeviceId;
	}

	function status(device: Device): string {
		const state = playback.state;
		if (device.id === realtime.deviceId && stale) return 'Not connected';
		if (!stale && device.id === playback.moving?.deviceId) return 'Starting…';
		if (device.id === state?.activeDeviceId) {
			// As the player bar shows the device playing, what was just asked of it included; while
			// offline, as it was last heard of.
			const said = stale
				? state.status === 'playing'
					? 'Was playing'
					: 'Was paused'
				: controls.playing
					? 'Playing'
					: 'Paused';
			const track = controls.track;
			const title = track && track.id === state.trackId ? track.title : null;
			return title ? `${said} · ${title}` : said;
		}
		if (!device.connected) return 'Not connected';
		return stale ? 'Was connected' : 'Connected';
	}

	/**
	 * A button gone once it has done its job leaves focus on the list, not lost to the page; one
	 * that never had focus leaves it where it is.
	 */
	function keepFocus(button: HTMLElement) {
		let focused = false;
		const check = () => (focused = document.activeElement === button);
		button.addEventListener('focus', check);
		// Not at once, since Chrome blurs a button as it goes, before the teardown below.
		button.addEventListener('blur', () => queueMicrotask(check));
		return () => {
			if (focused && document.activeElement === document.body) list?.focus();
		};
	}
</script>

{#if devices.length === 0}
	<p class="empty muted">
		{realtime.connected ? 'Loading…' : 'Your devices show once connected.'}
	</p>
{:else}
	<ul bind:this={list} aria-label="Devices" tabindex="-1">
		{#each devices as device (device.id)}
			<li class:off={!device.connected}>
				<span class="icon" class:playing={device.id === playback.state?.activeDeviceId}>
					<Icon name={device.type} size={22} />
				</span>
				<span class="meta">
					<span class="name">{device.name}</span>
					<span class="status muted">
						{device.id === realtime.deviceId ? 'This device · ' : ''}{status(device)}
					</span>
				</span>
				{#if device.id === realtime.deviceId && takeable}
					<button
						type="button"
						class="action"
						{@attach keepFocus}
						onclick={() => controls.playHere()}
					>
						Play here
					</button>
				{:else if movable(device)}
					<!-- One move at a time; kept while it goes, so focus stays where it was. -->
					<button
						type="button"
						class="action"
						{@attach keepFocus}
						aria-label="Play there, on {device.name}"
						aria-disabled={playback.moving !== null}
						onclick={() => {
							if (!playback.moving) playback.transfer(device.id);
						}}
					>
						Play there
					</button>
				{/if}
			</li>
		{/each}
	</ul>
	<p class="visually-hidden" role="status">
		{holder
			? `Playback is on ${holder.id === realtime.deviceId ? 'this device' : holder.name}`
			: ''}
	</p>
{/if}

<style>
	.empty {
		margin: var(--space-4) 0 0;
	}

	ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		width: 100%;
	}

	li {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2) 0;
	}

	.icon {
		display: grid;
		place-items: center;
		width: 40px;
		height: 40px;
		flex-shrink: 0;
		border-radius: var(--radius-m);
		background: var(--surface-hover);
		color: var(--text-muted);
	}

	.icon.playing {
		background: var(--accent-soft);
		color: var(--accent);
	}

	/* Not available, said in words; only the icon fades, so the text stays readable. */
	.off .icon {
		opacity: 0.5;
	}

	.off .name {
		color: var(--text-muted);
	}

	.meta {
		display: flex;
		flex: 1;
		flex-direction: column;
		min-width: 0;
	}

	.action {
		flex-shrink: 0;
		min-height: 32px;
		padding: 0 var(--space-3);
		border: 1px solid var(--accent);
		border-radius: 999px;
		background: transparent;
		color: var(--accent);
		font: inherit;
		font-size: 13px;
		font-weight: 600;
		cursor: pointer;
	}

	.action:hover {
		background: var(--accent-soft);
	}

	.action[aria-disabled='true'] {
		background: transparent;
		opacity: 0.5;
		cursor: default;
	}

	.name {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.status {
		font-size: 13px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
