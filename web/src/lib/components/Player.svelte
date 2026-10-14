<script lang="ts">
	import { resolve } from '$app/paths';
	import { formatDuration } from '$lib/format';
	import { panels } from '$lib/panels.svelte';
	import { playback } from '$lib/playback.svelte';
	import { player } from '$lib/player.svelte';
	import { realtime } from '$lib/realtime.svelte';
	import ArtistLinks from './ArtistLinks.svelte';
	import Artwork from './Artwork.svelte';
	import Icon from './Icon.svelte';
	import Waveform from './Waveform.svelte';

	// Another device's playback is shown here, to be picked up (`requirements/realtime.md` §2, §6).
	const remote = $derived(playback.shows(player.playing || player.finished));
	const track = $derived(remote ? playback.track : player.current);
	const playing = $derived(remote ? playback.state?.status === 'playing' : player.playing);
	const duration = $derived(remote ? (track?.durationUs ?? 0) / 1e6 : player.duration);
	const where = $derived(
		playback.state?.status === 'stopped'
			? 'Stopped'
			: realtime.connected
				? `On ${playback.deviceName}`
				: `Last on ${playback.deviceName}`
	);
	let now = $state(Date.now());
	$effect(() => {
		if (!remote || !playing) return;
		const timer = setInterval(() => (now = Date.now()), 250);
		return () => clearInterval(timer);
	});
	const position = $derived(remote ? playback.positionAt(now) : player.position);

	/** Whether what the other device was playing can play here: it is of this device's library. */
	const pickable = $derived(
		playback.track !== null && playback.state?.libraryId === player.libraryId
	);

	/** Plays here what the other device was playing, from where it got to. */
	function playHere() {
		const { track, state } = playback;
		if (track && state?.libraryId) {
			player.pickUp(state.libraryId, track, playback.positionAt(Date.now()));
		}
	}
	let lastVolume = 1;
	let innerWidth = $state(1024);
	const compact = $derived(innerWidth < 768);

	function toggleMute() {
		if (player.volume > 0) {
			lastVolume = player.volume;
			player.setVolume(0);
		} else {
			player.setVolume(lastVolume || 1);
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
				{#if remote}
					<span class="where">
						<span class="device">{where}</span>
						{#if pickable}
							<span aria-hidden="true">·</span>
							<button type="button" class="link" onclick={playHere}>Play here</button>
						{/if}
					</span>
				{/if}
			</div>
		{:else if remote}
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
				disabled={!track || remote}
				onclick={() => player.previous()}
			>
				<Icon name="previous" size={18} />
			</button>
			<button
				type="button"
				class="play"
				aria-label={playing ? 'Pause' : 'Play'}
				disabled={!track || remote}
				onclick={() => player.toggle()}
			>
				<Icon name={playing ? 'pause' : 'play'} size={18} />
			</button>
			<button
				type="button"
				class="icon-btn skip"
				aria-label="Next"
				disabled={!track || remote}
				onclick={() => player.next()}
			>
				<Icon name="next" size={18} />
			</button>
		</div>
		<div class="progress">
			<span class="time muted tabular">{formatDuration(position * 1e6)}</span>
			<div class="wave-slot">
				<Waveform
					height={compact ? 20 : 40}
					points={track ? (remote ? playback.waveform : player.waveform) : null}
					{position}
					duration={track ? duration : 0}
					seekable={!remote}
					onseek={(s) => player.seek(s)}
				/>
			</div>
			<span class="time muted tabular">{formatDuration(duration * 1e6)}</span>
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
			onclick={() => (panels.lyrics = !panels.lyrics)}
		>
			<Icon name="lyrics" size={18} />
		</button>
		<span class="volume">
			<button
				type="button"
				class="icon-btn"
				aria-label={player.volume > 0 ? 'Mute' : 'Unmute'}
				onclick={toggleMute}
			>
				<Icon name={player.volume > 0 ? 'volume' : 'mute'} size={18} />
			</button>
			<input
				type="range"
				min="0"
				max="1"
				step="0.01"
				aria-label="Volume"
				value={player.volume}
				oninput={(e) => player.setVolume(Number(e.currentTarget.value))}
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

	/* A long device name gives way, never the button. */
	.device {
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.link {
		flex-shrink: 0;
		min-height: 24px;
		padding: 0;
		border: 0;
		background: none;
		color: inherit;
		font: inherit;
		font-weight: 600;
		text-decoration: underline;
		cursor: pointer;
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
