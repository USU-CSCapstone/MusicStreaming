// Local playback: one audio element playing through a context of tracks.
// Queue editing, sync across devices, and Shift come later (`requirements/queue.md`,
// `requirements/realtime.md`).

import { audioUrl, getPlaybackInfo, getWaveform, imageUrl } from './api/client';
import type { TrackSummary } from './api/types';
import { albumTitle, artistName } from './format';
import { firstPlayable, nextPlayable, previousTarget } from './queue';
import { decodeWaveform } from './waveform';

class Player {
	libraryId = '';

	tracks = $state<TrackSummary[]>([]);
	index = $state(-1);
	current = $derived<TrackSummary | null>(this.tracks[this.index] ?? null);

	playing = $state(false);
	/** Seconds. */
	position = $state(0);
	/** Seconds; the tagged duration until the audio reports its own. */
	duration = $state(0);
	volume = $state(1);
	/** Null while loading or until the track is analyzed; the progress bar is then plain. */
	waveform = $state<Uint8Array | null>(null);

	#audio: HTMLAudioElement | null = null;
	/** Bumped on every load, so a slow response for an earlier track is ignored. */
	#load = 0;

	/** Play `tracks` as the context, starting at `index`. */
	play(tracks: TrackSummary[], index: number) {
		const start = firstPlayable(tracks, index);
		if (start === null) return;
		this.tracks = tracks;
		this.#go(start);
	}

	toggle() {
		const audio = this.#audio;
		if (!audio || !this.current) return;
		if (audio.paused) void audio.play().catch(() => {});
		else audio.pause();
	}

	next() {
		const i = nextPlayable(this.tracks, this.index, 1);
		if (i === null) {
			// End of the context: stop at the end rather than wrapping.
			this.#audio?.pause();
			return;
		}
		this.#go(i);
	}

	previous() {
		const target = previousTarget(this.tracks, this.index, this.position);
		if (target.restart) this.seek(0);
		else this.#go(target.index);
	}

	seek(seconds: number) {
		const s = Math.max(0, Math.min(seconds, this.duration || 0));
		this.position = s;
		if (this.#audio) this.#audio.currentTime = s;
	}

	setVolume(v: number) {
		this.volume = Math.max(0, Math.min(1, v));
		if (this.#audio) this.#audio.volume = this.volume;
	}

	#go(index: number) {
		this.index = index;
		const track = this.tracks[index];
		const load = ++this.#load;
		const audio = this.#ensureAudio();
		audio.pause();
		this.position = 0;
		this.duration = track.durationUs / 1_000_000;
		this.waveform = null;
		this.#mediaSession(track);

		const lib = this.libraryId;
		getPlaybackInfo(fetch, lib, track.id)
			.then((info) => {
				if (load !== this.#load) return;
				audio.src = audioUrl(lib, track.id, info.variant);
				return audio.play();
			})
			.catch(() => {
				// A track that cannot play is skipped in place, without a message (`requirements/conventions.md` §6).
				if (load === this.#load) this.next();
			});
		getWaveform(fetch, lib, track.id)
			.then((w) => {
				if (load === this.#load && w) this.waveform = decodeWaveform(w);
			})
			.catch(() => {});
	}

	#ensureAudio(): HTMLAudioElement {
		if (this.#audio) return this.#audio;
		const audio = new Audio();
		audio.preload = 'auto';
		audio.volume = this.volume;
		audio.addEventListener('play', () => (this.playing = true));
		audio.addEventListener('pause', () => (this.playing = false));
		audio.addEventListener('timeupdate', () => (this.position = audio.currentTime));
		audio.addEventListener('durationchange', () => {
			if (Number.isFinite(audio.duration)) this.duration = audio.duration;
		});
		audio.addEventListener('ended', () => this.next());
		audio.addEventListener('error', () => {
			if (audio.src) this.next();
		});
		this.#audio = audio;

		if ('mediaSession' in navigator) {
			const ms = navigator.mediaSession;
			ms.setActionHandler('play', () => this.toggle());
			ms.setActionHandler('pause', () => this.toggle());
			ms.setActionHandler('previoustrack', () => this.previous());
			ms.setActionHandler('nexttrack', () => this.next());
			ms.setActionHandler('seekto', (d) => d.seekTime !== undefined && this.seek(d.seekTime));
		}
		return audio;
	}

	/** Lock screen, notification shade, and media keys (`requirements/playback.md` §7). */
	#mediaSession(track: TrackSummary) {
		if (!('mediaSession' in navigator)) return;
		const image = track.album.image;
		navigator.mediaSession.metadata = new MediaMetadata({
			title: track.title,
			artist: track.artists.map((a) => artistName(a.name)).join(', '),
			album: albumTitle(track.album.title),
			artwork: image
				? [{ src: imageUrl(this.libraryId, image, 512), sizes: '512x512', type: 'image/jpeg' }]
				: []
		});
	}
}

export const player = new Player();
