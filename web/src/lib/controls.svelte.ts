// What the player bar shows and its controls act on: this device's own player, or the playback
// another device holds, which they then control (`requirements/realtime.md` §3). The bar, the
// space bar, and the lyrics all go through here, so they agree on which. The lock screen and
// media keys show this device's own player, and so act on it (`player.svelte.ts`).

import { playback } from './playback.svelte';
import { player } from './player.svelte';
import { queue } from './queue/client.svelte';

class Controls {
	/**
	 * Whether this shows another device's playback rather than this one's: never while this one
	 * plays, or stopped at the end of what it played, whether or not the server has heard yet;
	 * otherwise when another device holds playback, or playback stopped elsewhere, so any device
	 * can pick it up from where it stopped (`requirements/realtime.md` §6).
	 */
	remote = $derived(
		!(player.playing || player.finished) &&
			(playback.elsewhere ||
				(playback.state?.status === 'stopped' && playback.state.trackId != null))
	);
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

	/** Whether what the other device was playing can play here: it is of this device's library and this device holds the queue. */
	pickable = $derived(
		playback.track !== null &&
			playback.state?.libraryId === player.libraryId &&
			queue.queueId !== null
	);

	/** Plays here what the other device was playing, from where it got to, or says it could not. */
	playHere() {
		if (!this.pickable) return;
		void player.pickUp().catch(() => playback.fail('Could not play that here.'));
	}
}

export const controls = new Controls();
