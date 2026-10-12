// Local playback: one audio element playing through a context of tracks. It reports its
// progress to the server and stops when another device takes over (`progress.ts`), and plays
// in one tab of this browser at a time, since every tab is the same device to the server.
// Queue editing and Shift come later (`requirements/queue.md`, `requirements/realtime.md`).

import { audioUrl, getPlaybackInfo, getWaveform, imageUrl, recordPlays } from './api/client';
import type { PlayContext, TrackSummary } from './api/types';
import { albumTitle, artistName } from './format';
import { PlayRecorder } from './plays';
import { Progress } from './progress';
import { firstPlayable, nextPlayable, previousTarget } from './queue';
import { realtime } from './realtime.svelte';
import { decodeWaveform } from './waveform';

/** How often a playing player says where it is, so a device picking up is never far behind. */
const REPORT_EVERY_MS = 10_000;
/** How long the volume rests before it is reported, so dragging the slider is one report. */
const VOLUME_SETTLES_MS = 250;

type Status = 'playing' | 'paused' | 'stopped' | 'buffering';

class Player {
	libraryId = '';

	tracks = $state<TrackSummary[]>([]);
	/** Where `tracks` came from, which each play records. */
	context: PlayContext = { type: 'library' };
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
	/** The load whose track has started playing, so pausing and resuming is one play. */
	#playStarted = 0;
	#plays = new PlayRecorder((items, keepalive) => recordPlays(fetch, items, keepalive));
	#progress = new Progress((report) => realtime.send({ type: 'progress', report }));
	/** Waiting for audio to arrive, though not paused. */
	#buffering = false;
	/** Between choosing a track and its audio starting to play. */
	#loading = false;
	/** The audio is of a track before the current one, whose loading a takeover cut short. */
	#stale = false;
	#volumeReport: ReturnType<typeof setTimeout> | undefined;
	/** This browser's other tabs, told when playback starts in this one. */
	#tabs = typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel('jewelcase.player');

	constructor() {
		realtime.on((message) => {
			if (message.type === 'hello') this.#progress.connected();
			if (message.type !== 'player' || !realtime.deviceId) return;
			const playing = this.playing || this.#loading;
			const action = this.#progress.heard(realtime.deviceId, message.state, playing);
			if (action === 'stop') this.#yield();
			else if (action === 'report') this.#report();
		});
		this.#tabs?.addEventListener('message', () => this.#yield());
	}

	/** Another device or tab took over: this one stops where it is, and can take it back. */
	#yield() {
		if (!this.playing && !this.#loading) return;
		// A track still loading must not start once its audio arrives.
		this.#load++;
		if (this.#loading) this.#stale = true;
		this.#loading = false;
		this.#audio?.pause();
		this.#plays.end('stopped');
	}

	/** Play `tracks` from `context`, starting at `index`. */
	play(tracks: TrackSummary[], index: number, context: PlayContext = { type: 'library' }) {
		const start = firstPlayable(tracks, index);
		if (start === null) return;
		this.tracks = tracks;
		this.context = context;
		this.#started();
		this.#go(start);
	}

	toggle() {
		const audio = this.#audio;
		if (!audio || !this.current) return;
		if (audio.paused) {
			this.#started();
			if (this.#stale) this.#go(this.index);
			else void audio.play().catch(() => {});
		} else {
			this.#progress.paused();
			audio.pause();
		}
	}

	next() {
		if (nextPlayable(this.tracks, this.index, 1) !== null) this.#started();
		this.#next();
	}

	previous() {
		const target = previousTarget(this.tracks, this.index, this.position);
		// Back to the start of this track takes nothing over; it leaves it playing or not.
		if (target.restart) this.seek(0);
		else {
			this.#started();
			this.#go(target.index);
		}
	}

	seek(seconds: number) {
		const s = Math.max(0, Math.min(seconds, this.duration || 0));
		this.position = s;
		if (this.#audio) this.#audio.currentTime = s;
		this.#report();
	}

	setVolume(v: number) {
		this.volume = Math.max(0, Math.min(1, v));
		if (this.#audio) this.#audio.volume = this.volume;
		clearTimeout(this.#volumeReport);
		this.#volumeReport = setTimeout(() => this.#report(), VOLUME_SETTLES_MS);
	}

	/** The user started or moved playback here, which takes it over from anywhere else. */
	#started() {
		this.#progress.started();
		this.#tabs?.postMessage('started');
	}

	/** On to the next track, or stopped at the end of the context rather than wrapping. */
	#next() {
		const i = nextPlayable(this.tracks, this.index, 1);
		if (i === null) {
			this.#audio?.pause();
			this.#plays.end('skipped');
			this.#report('stopped');
			return;
		}
		this.#go(i);
	}

	/** Tells the server where the player is: `status`, or as the audio element says. */
	#report(status?: Status) {
		const track = this.current;
		if (!track || !realtime.deviceId) return;
		const audio = this.#audio;
		status ??= !audio || audio.paused ? 'paused' : this.#buffering ? 'buffering' : 'playing';
		this.#progress.report(realtime.deviceId, {
			// Its place in the context, until the server keeps the queue.
			itemId: String(this.index),
			libraryId: this.libraryId,
			trackId: track.id,
			status,
			positionMs: Math.round(this.position * 1000),
			volume: this.volume
		});
	}

	#go(index: number) {
		this.index = index;
		const track = this.tracks[index];
		const load = ++this.#load;
		const audio = this.#ensureAudio();
		// One that played to its end has ended its play already.
		this.#plays.end('skipped');
		audio.pause();
		this.position = 0;
		this.duration = track.durationUs / 1_000_000;
		this.waveform = null;
		this.#mediaSession(track);
		this.#loading = true;
		this.#stale = false;
		this.#report('buffering');

		const lib = this.libraryId;
		getPlaybackInfo(fetch, lib, track.id)
			.then((info) => {
				if (load !== this.#load) return;
				audio.src = audioUrl(lib, track.id, info.variant);
				return audio.play();
			})
			.catch(() => {
				// A track that cannot play is skipped in place, without a message (`requirements/conventions.md` §6).
				if (load !== this.#load) return;
				this.#loading = false;
				this.#next();
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
		audio.addEventListener('playing', () => {
			this.#buffering = false;
			this.#loading = false;
			this.#report();
			if (this.#playStarted === this.#load || !this.current) return;
			this.#playStarted = this.#load;
			this.#plays.start(this.libraryId, this.current.id, this.context, audio.currentTime);
		});
		audio.addEventListener('waiting', () => {
			this.#buffering = true;
			this.#report();
		});
		audio.addEventListener('pause', () => {
			this.playing = false;
			if (!audio.ended) this.#plays.paused();
			// Changing track pauses the one before; the new one is reported as loading instead.
			if (!audio.ended && !this.#loading) this.#report();
		});
		setInterval(() => {
			if (this.playing) this.#report();
		}, REPORT_EVERY_MS);
		audio.addEventListener('timeupdate', () => {
			this.position = audio.currentTime;
			this.#plays.heard(audio.currentTime);
		});
		audio.addEventListener('seeking', () => this.#plays.seeked(audio.currentTime));
		audio.addEventListener('durationchange', () => {
			if (Number.isFinite(audio.duration)) this.duration = audio.duration;
		});
		audio.addEventListener('ended', () => {
			this.#plays.heard(audio.currentTime);
			this.#plays.end('finished');
			this.#next();
		});
		// Closing the page stops what is playing. The report must outlive the page.
		addEventListener('pagehide', () => this.#plays.end('stopped', true));
		audio.addEventListener('error', () => {
			if (audio.src) this.#next();
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
