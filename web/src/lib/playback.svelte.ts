// The account's playback as the server last told it (`requirements/realtime.md` §2, §4): which
// device plays, what, and where, and the names of the account's devices. A device that is not
// the one playing shows this instead of its own player, so the user can see where the music
// went, control it, and take it back. Any device can move playback to another (§3).
//
// A command to the device playing shows its effect at once and is set right by the next
// state; one that fails is undone, and says why (§5).

import { flushSync } from 'svelte';
import { getTrack, getWaveform } from './api/client';
import type { Device, PlayerCommand, PlayerState, TrackSummary } from './api/types';
import { realtime } from './realtime.svelte';
import { decodeWaveform } from './waveform';

/** How long a failed command's message stays. */
const ERROR_FOR_MS = 5_000;
/** How long the volume rests before it is sent, so dragging the slider is one command. */
const VOLUME_SETTLES_MS = 250;

/** A command as sent, but for its ID. */
type Command = PlayerCommand extends infer C
	? C extends unknown
		? Omit<C, 'commandId'>
		: never
	: never;

class Playback {
	/** The latest state, or null before the server has sent one. */
	state = $state<PlayerState | null>(null);
	devices = $state<Device[]>([]);
	/** The track the state names, once fetched; the one before until then. */
	track = $state<TrackSummary | null>(null);
	waveform = $state<Uint8Array | null>(null);
	/** Why the latest command, or picking up here, failed, for a few seconds. */
	error = $state<string | null>(null);
	/** Playback being moved to another device, until it takes over or says it cannot. */
	moving = $state<{ deviceId: string; commandId: string } | null>(null);
	/** What commands just asked of the device playing, shown until the server says how it went. */
	#asked = $state<Partial<PlayerState>>({});
	/** The state as shown: the server's, with what was just asked of it. */
	shown = $derived(this.state && { ...this.state, ...this.#asked });
	/** The IDs of commands sent and not yet answered. */
	#waiting = new Set<string>();
	#errorTimer: ReturnType<typeof setTimeout> | undefined;
	#volumeTimer: ReturnType<typeof setTimeout> | undefined;

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
			// The answer to a command sent over a connection now gone is lost with it.
			if (message.type === 'hello') this.#forget();
			if (message.type === 'devices') this.devices = message.devices;
			if (message.type === 'commandResult') this.#answered(message.result);
			if (message.type !== 'player') return;
			// What was asked of one device means nothing to another that has taken over.
			if (message.state.activeDeviceId !== this.state?.activeDeviceId) this.#forget();
			this.state = message.state;
			// A state from before a command was carried out may still be on its way until then.
			const asking = this.#waiting.size > 0 || this.#volumeTimer !== undefined;
			if (!asking) this.#asked = {};
			// The device playing has its own track; any other fetches the one it plays.
			if (message.state.activeDeviceId !== realtime.deviceId) this.#fetch(message.state);
		});
	}

	/**
	 * Seconds into the track at `now`, as this browser's clock reads, never past its end. It
	 * moves on only while connected: one that is not cannot know it is still playing (§6).
	 */
	positionAt(now: number): number {
		const state = this.shown;
		if (!state) return 0;
		let ms = state.positionMs;
		const at = Date.parse(state.positionAt);
		// Moving on from where it was when the server last heard, by the server's clock.
		// Never back: a `now` from before the state was true is as good as when it was.
		if (state.status === 'playing' && realtime.connected && Number.isFinite(at)) {
			ms += Math.max(0, now + realtime.clockOffset - at);
		}
		// Not the end of a track from before, while the one it names is on its way.
		const track = this.track?.id === state.trackId ? this.track : null;
		const end = track ? track.durationUs / 1000 : Infinity;
		return Math.max(0, Math.min(ms, end)) / 1000;
	}

	play() {
		this.#send({ type: 'play' }, { status: 'playing', ...this.#here() });
	}

	pause() {
		this.#send({ type: 'pause' }, { status: 'paused', ...this.#here() });
	}

	next() {
		this.#send({ type: 'next' });
	}

	previous() {
		this.#send({ type: 'previous' });
	}

	seek(seconds: number) {
		const positionMs = Math.round(seconds * 1000);
		this.#send({ type: 'seek', positionMs }, { positionMs, positionAt: this.#serverNow() });
	}

	/** Moves playback to `deviceId`, playing there unless it is paused, or waking it if stopped. */
	transfer(deviceId: string) {
		const play = this.shown?.status !== 'paused';
		const commandId = this.#send({ type: 'transfer', deviceId, play });
		this.moving = commandId ? { deviceId, commandId } : null;
	}

	/** Shows `volume` at once, and sends it once the slider rests. */
	setVolume(volume: number) {
		// The device it is turned up or down on, wherever playback goes meanwhile.
		const deviceId = this.state?.activeDeviceId;
		if (!deviceId) return;
		this.#asked = { ...this.#asked, volume };
		clearTimeout(this.#volumeTimer);
		this.#volumeTimer = setTimeout(() => {
			this.#volumeTimer = undefined;
			this.#send({ type: 'setVolume', deviceId, volume }, { volume });
		}, VOLUME_SETTLES_MS);
	}

	/** Lets go of every command still waiting, and what they asked for. */
	#forget() {
		this.#waiting.clear();
		this.#asked = {};
		this.moving = null;
		clearTimeout(this.#volumeTimer);
		this.#volumeTimer = undefined;
	}

	/** Where playback is now, as a state would say it. */
	#here(): Partial<PlayerState> {
		return {
			positionMs: Math.round(this.positionAt(Date.now()) * 1000),
			positionAt: this.#serverNow()
		};
	}

	#serverNow(): string {
		return new Date(Date.now() + realtime.clockOffset).toISOString();
	}

	/**
	 * Sends `command` to the device playing, or the one a transfer names, showing `change` until
	 * the server answers. Answers its ID, or null if it could not be sent.
	 */
	#send(command: Command, change: Partial<PlayerState> = {}): string | null {
		const commandId = crypto.randomUUID();
		if (
			!realtime.send({ type: 'playerCommand', command: { ...command, commandId } as PlayerCommand })
		) {
			this.#asked = {};
			this.fail('Not connected to the server.');
			return null;
		}
		this.#waiting.add(commandId);
		this.#asked = { ...this.#asked, ...change };
		return commandId;
	}

	#answered(result: { commandId: string; status: string; reason?: string | null }) {
		if (result.status === 'pending' || !this.#waiting.delete(result.commandId)) return;
		if (result.commandId === this.moving?.commandId) this.moving = null;
		if (result.status === 'failed') {
			// What it asked for did not happen: back to what the server says did.
			this.#asked = {};
			this.fail(result.reason ?? 'That did not work.');
		}
	}

	/** Shows why something asked of playback did not happen, for a few seconds. */
	fail(reason: string) {
		// The same reason again is cleared first, so it is shown, and said, again.
		if (this.error === reason) {
			this.error = null;
			flushSync();
		}
		this.error = reason;
		clearTimeout(this.#errorTimer);
		this.#errorTimer = setTimeout(() => (this.error = null), ERROR_FOR_MS);
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
