// What the player bar shows and its controls act on: this device's own player, or the playback
// another device holds, which they then control (`requirements/realtime.md` §3). The bar, the
// space bar, the lock screen, and the lyrics all go through here, so they agree on which.

import { playback } from './playback.svelte';
import { player } from './player.svelte';

class Controls {
	/** Whether this shows another device's playback rather than this one's. */
	remote = $derived(playback.shows(player.playing || player.finished));
	/** Whether that playback stopped with no device holding it, so playing picks it up here. */
	stopped = $derived(this.remote && !playback.elsewhere);
	track = $derived(this.remote ? playback.track : player.current);
	playing = $derived(this.remote ? playback.shown?.status === 'playing' : player.playing);
	/** Seconds. */
	duration = $derived(this.remote ? (this.track?.durationUs ?? 0) / 1e6 : player.duration);
	waveform = $derived(this.remote ? playback.waveform : player.waveform);
	volume = $derived(this.remote ? (playback.shown?.volume ?? 1) : player.volume);
	/** Now, as often as a position moving on elsewhere needs it. */
	#now = $state(Date.now());
	/** Seconds. */
	position = $derived(this.remote ? playback.positionAt(this.#now) : player.position);

	constructor() {
		$effect.root(() => {
			$effect(() => {
				if (!this.remote || !this.playing) return;
				// Not from when it last stopped ticking.
				this.#now = Date.now();
				const timer = setInterval(() => (this.#now = Date.now()), 250);
				return () => clearInterval(timer);
			});
		});
		player.actions = this;
	}

	play() {
		if (!this.remote) void player.resume().catch(() => {});
		else if (this.stopped) this.playHere();
		else playback.play();
	}

	pause() {
		if (!this.remote) player.pause();
		else if (!this.stopped) playback.pause();
	}

	toggle() {
		if (!this.remote) player.toggle();
		else if (this.stopped) this.playHere();
		else if (this.playing) playback.pause();
		else playback.play();
	}

	next() {
		if (!this.remote) player.next();
		else if (!this.stopped) playback.next();
	}

	previous() {
		if (!this.remote) player.previous();
		else if (!this.stopped) playback.previous();
	}

	seek(seconds: number) {
		if (!this.remote) player.seek(seconds);
		else if (!this.stopped) playback.seek(seconds);
	}

	setVolume(volume: number) {
		if (!this.remote) player.setVolume(volume);
		else if (!this.stopped) playback.setVolume(volume);
	}

	/** Whether what the other device was playing can play here: it is of this device's library. */
	pickable = $derived(playback.track !== null && playback.state?.libraryId === player.libraryId);

	/** Plays here what the other device was playing, from where it got to. */
	playHere() {
		const { track, state } = playback;
		if (track && state?.libraryId) {
			player.pickUp(state.libraryId, track, playback.positionAt(Date.now()));
		}
	}
}

export const controls = new Controls();
