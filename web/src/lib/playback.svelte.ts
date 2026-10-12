// The account's playback as the server last told it (`requirements/realtime.md` §2, §4): which
// device plays, what, and where, and the names of the account's devices. A device that is not
// the one playing shows this instead of its own player, so the user can see where the music
// went and take it back.

import { getTrack, getWaveform } from './api/client';
import type { Device, PlayerState, TrackSummary } from './api/types';
import { realtime } from './realtime.svelte';
import { decodeWaveform } from './waveform';

class Playback {
	/** The latest state, or null before the server has sent one. */
	state = $state<PlayerState | null>(null);
	devices = $state<Device[]>([]);
	/** The track the state names, once fetched; the one before until then. */
	track = $state<TrackSummary | null>(null);
	waveform = $state<Uint8Array | null>(null);

	/** Whether another device holds playback, playing or paused. */
	elsewhere = $derived(
		this.state?.activeDeviceId != null && this.state.activeDeviceId !== realtime.deviceId
	);

	/** The name of the device holding playback, as the user knows it. */
	deviceName = $derived(
		this.devices.find((device) => device.id === this.state?.activeDeviceId)?.name ??
			'another device'
	);

	/** The track being fetched, so it is fetched once, and again after failing. */
	#fetching: string | null = null;

	constructor() {
		realtime.on((message) => {
			if (message.type === 'devices') this.devices = message.devices;
			if (message.type !== 'player') return;
			this.state = message.state;
			// The device playing has its own track; any other fetches the one it plays.
			if (message.state.activeDeviceId !== realtime.deviceId) this.#fetch(message.state);
		});
	}

	/**
	 * Whether a player that is `playing` here shows this instead of its own: when another
	 * device holds playback, or it stopped elsewhere and this one is not playing, so any device
	 * can pick it up from where it stopped (`requirements/realtime.md` §6).
	 */
	shows(playing: boolean): boolean {
		const stopped = this.state?.status === 'stopped' && this.state.trackId != null;
		return this.elsewhere || (stopped && !playing);
	}

	/**
	 * Seconds into the track at `now`, as this browser's clock reads, never past its end. It
	 * moves on only while connected: one that is not cannot know it is still playing (§6).
	 */
	positionAt(now: number): number {
		const state = this.state;
		if (!state) return 0;
		let ms = state.positionMs;
		const at = Date.parse(state.positionAt);
		// Moving on from where it was when the server last heard, by the server's clock.
		if (state.status === 'playing' && realtime.connected && Number.isFinite(at)) {
			ms += now + realtime.clockOffset - at;
		}
		const end = this.track ? this.track.durationUs / 1000 : Infinity;
		return Math.max(0, Math.min(ms, end)) / 1000;
	}

	#fetch({ libraryId, trackId }: PlayerState) {
		if (!libraryId || !trackId || trackId === this.track?.id || trackId === this.#fetching) {
			return;
		}
		this.#fetching = trackId;
		const current = () => this.#fetching === trackId;
		getTrack(fetch, libraryId, trackId)
			.then((track) => {
				if (!current()) return;
				this.track = track;
				this.waveform = null;
				this.#fetching = null;
				return getWaveform(fetch, libraryId, trackId);
			})
			.then((w) => {
				if (w && this.track?.id === trackId) this.waveform = decodeWaveform(w);
			})
			// Tried again with the next state that names it.
			.catch(() => {
				if (current()) this.#fetching = null;
			});
	}
}

export const playback = new Playback();
