<script lang="ts">
	import { resolve } from '$app/paths';
	import { formatDuration } from '$lib/format';
	import { controls } from '$lib/controls.svelte';
	import { panels } from '$lib/panels.svelte';
	import { playback } from '$lib/playback.svelte';
	import { realtime } from '$lib/realtime.svelte';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import Icon from './Icon.svelte';
	import Waveform from './Waveform.svelte';

	// This device's player, or another device's playback to control and pick up
	// (`requirements/realtime.md` §2, §3, §6).
	const remote = $derived(controls.remote);
	const track = $derived(controls.track);
	const playing = $derived(controls.playing);
	/** Where the music plays, when another device plays it; stopped, it plays nowhere. */
	const where = $derived(
		controls.stopped
			? null
			: realtime.connected
				? `On ${playback.deviceName}`
				: `Last on ${playback.deviceName}`
	);
	const devicesLabel = $derived(
		playback.elsewhere ? `Devices, playing on ${playback.deviceName}` : 'Devices'
	);
	let lastVolume = 1;
	let innerWidth = $state(1024);
	const compact = $derived(innerWidth < 768);

	function toggleMute() {
		if (controls.volume > 0) {
			lastVolume = controls.volume;
			controls.setVolume(0);
		} else {
			controls.setVolume(lastVolume || 1);
		}
	}
</script>

<svelte:window bind:innerWidth />

<!-- Shown on a phone while offline even with nothing playing, so the user knows. -->
<section class="player" class:idle={!track && !remote && !realtime.offline} aria-label="Player">
	<div class="now">
		{#if track}
			<a class="art" href={resolve('/albums/[id]', { id: track.album.id })} aria-label="Open album">
				<Artwork image={track.album.image} seed={track.album.id} size={56} />
			</a>
			<div class="meta">
				<span class="title">{track.title}</span>
				<span class="muted artists"><ArtistLinks artists={track.artists} /></span>
				{#if remote && (playback.error || where)}
					<span class="where">
						{#if playback.error}
							<span class="device error" role="alert" title={playback.error}>{playback.error}</span>
						{:else}
							<span class="device">{where}</span>
						{/if}
					</span>
				{/if}
			</div>
		{:else if remote && where}
			<span class="muted">{where}</span>
		{:else}
			<span class="muted">Nothing playing</span>
		{/if}
	</div>

	<div class="center">
		<div class="controls">
			<button
				type="button"
				class="icon-btn skip"
				aria-label="Previous"
				disabled={!track || controls.stopped}
				onclick={() => controls.previous()}
			>
				<Icon name="previous" size={18} />
			</button>
			<button
				type="button"
				class="play"
				aria-label={playing ? 'Pause' : 'Play'}
				disabled={!track || (controls.stopped && !controls.pickable)}
				onclick={() => controls.toggle()}
			>
				<Icon name={playing ? 'pause' : 'play'} size={18} />
			</button>
			<button
				type="button"
				class="icon-btn skip"
				aria-label="Next"
				disabled={!track || controls.stopped}
				onclick={() => controls.next()}
			>
				<Icon name="next" size={18} />
			</button>
		</div>
		<div class="progress">
			<span class="time muted tabular">{formatDuration(controls.position * 1e6)}</span>
			<div class="wave-slot">
				<Waveform
					height={compact ? 20 : 40}
					points={track ? controls.waveform : null}
					position={controls.position}
					duration={track ? controls.duration : 0}
					seekable={!controls.stopped}
					onseek={(s) => controls.seek(s)}
				/>
			</div>
			<span class="time muted tabular">{formatDuration(controls.duration * 1e6)}</span>
		</div>
	</div>

	<div class="side">
		<!-- Always there, so a screen reader announces the change. -->
		<span class="offline" role="status">{realtime.offline ? 'Offline' : ''}</span>
		<button
			type="button"
			class="icon-btn"
			class:on={panels.lyrics}
			aria-label="Lyrics"
			aria-pressed={panels.lyrics}
			title="Lyrics"
			onclick={() => {
				panels.lyrics = !panels.lyrics;
				panels.devices = false;
			}}
		>
			<Icon name="lyrics" size={18} />
		</button>
		<!-- Marked while another device plays, where the music can be taken back from. -->
		<button
			type="button"
			class="icon-btn"
			class:on={panels.devices}
			class:elsewhere={playback.elsewhere}
			aria-label={devicesLabel}
			aria-pressed={panels.devices}
			title={devicesLabel}
			onclick={() => {
				panels.devices = !panels.devices;
				panels.lyrics = false;
			}}
		>
			<Icon name="devices" size={18} />
		</button>
		<span class="volume">
			<button
				type="button"
				class="icon-btn"
				disabled={controls.stopped}
				aria-label={controls.volume > 0 ? 'Mute' : 'Unmute'}
				onclick={toggleMute}
			>
				<Icon name={controls.volume > 0 ? 'volume' : 'mute'} size={18} />
			</button>
			<input
				type="range"
				min="0"
				max="1"
				step="0.01"
				aria-label="Volume"
				disabled={controls.stopped}
				value={controls.volume}
				oninput={(e) => controls.setVolume(Number(e.currentTarget.value))}
			/>
		</span>
	</div>
</section>

<style>
	.player {
		display: grid;
		grid-template-columns: minmax(180px, 1fr) minmax(0, 2fr) minmax(140px, 1fr);
		align-items: center;
		gap: var(--space-5);
		padding: var(--space-3) var(--space-5);
		background: var(--surface);
		border-top: 1px solid var(--border);
	}

	.now {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		min-width: 0;
	}

	.art {
		width: 56px;
		flex-shrink: 0;
	}

	.meta {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.title {
		font-weight: 600;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.artists {
		display: flex;
		min-width: 0;
		font-size: 13px;
	}

	.where {
		display: flex;
		align-items: center;
		gap: var(--space-1);
		min-width: 0;
		color: var(--accent);
		font-size: 12px;
	}

	.device.error {
		color: var(--danger);
	}

	/* A long device name gives way, never the button. */
	.device {
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.center {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-1);
		min-width: 0;
	}

	.controls {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.progress {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		width: 100%;
		max-width: 720px;
	}

	.wave-slot {
		flex: 1;
		min-width: 0;
	}

	.time {
		width: 40px;
		font-size: 12px;
		text-align: center;
	}

	.icon-btn {
		display: grid;
		place-items: center;
		width: 34px;
		height: 34px;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
	}

	.icon-btn:hover:not(:disabled) {
		color: var(--text);
		background: var(--surface-hover);
	}

	.play {
		display: grid;
		place-items: center;
		width: 38px;
		height: 38px;
		border: 0;
		border-radius: 50%;
		background: var(--text);
		color: var(--surface);
		cursor: pointer;
	}

	.play:hover:not(:disabled) {
		transform: scale(1.05);
	}

	button:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.icon-btn.elsewhere {
		position: relative;
		color: var(--accent);
	}

	.icon-btn.elsewhere::after {
		content: '';
		position: absolute;
		bottom: 1px;
		left: 50%;
		width: 4px;
		height: 4px;
		border-radius: 50%;
		background: var(--accent);
		transform: translateX(-50%);
	}

	.icon-btn.on {
		color: var(--accent);
		background: var(--accent-soft);
	}

	.offline:empty {
		display: none;
	}

	.offline {
		padding: 2px var(--space-2);
		border: 1px solid var(--danger);
		border-radius: var(--radius-s);
		color: var(--danger);
		font-size: 12px;
		white-space: nowrap;
	}

	.volume {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.side {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--space-2);
	}

	input[type='range'] {
		width: 110px;
		accent-color: var(--accent);
	}

	/* Phone width: the song and play/pause in a row, the waveform as a thin strip below. */
	@media (max-width: 767px) {
		.player {
			grid-template-columns: minmax(0, 1fr) auto auto;
			grid-template-areas:
				'now controls side'
				'wave wave wave';
			gap: var(--space-1) var(--space-3);
			padding: var(--space-2) var(--space-3) var(--space-1);
		}

		.player.idle {
			display: none;
		}

		.now {
			grid-area: now;
		}

		.art {
			width: 44px;
		}

		.center {
			display: contents;
		}

		.controls {
			grid-area: controls;
		}

		.progress {
			grid-area: wave;
			max-width: none;
		}

		.side {
			grid-area: side;
		}

		.time,
		.volume {
			display: none;
		}
	}
</style>
