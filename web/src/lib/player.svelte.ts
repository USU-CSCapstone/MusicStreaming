// Local playback: one audio element playing through a context of tracks. It reports its
// progress to the server and stops when another device takes over (`progress.ts`), carries out
// commands from the user's other devices while it plays, and takes playback over when one moves
// it here (`remote.ts`). It plays in one tab of this browser at a time, since every tab is the
// same device to the server.
// Queue editing comes later (`requirements/queue.md`).

import {
	audioUrl,
	getPlaybackInfo,
	getTrack,
	getWaveform,
	imageUrl,
	recordPlays
} from './api/client';
import type { PlayContext, PlayerCommand, PlayerState, TrackSummary } from './api/types';
import { albumTitle, artistName } from './format';
import { playback } from './playback.svelte';
import { PlayRecorder } from './plays';
import { Progress } from './progress';
import { firstPlayable, nextPlayable, previousTarget } from './queue';
import { realtime } from './realtime.svelte';
import { carryOut, type Controls } from './remote';
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

	/** Whether it plays, or is to once the track's audio arrives. */
	playing = $state(false);
	/**
	 * Whether playback stopped here, at the end of what this player had, and has not started
	 * anywhere since: nothing holds playback, but this player is still the one to show.
	 */
	finished = $state(false);
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
	#plays = new PlayRecorder((items, keepalive) => recordPlays(fetch, items, keepalive));
	#progress = new Progress((report) => realtime.send({ type: 'progress', report }));
	/** Waiting for audio to arrive, though not paused. */
	#buffering = false;
	/** Between choosing a track and its audio starting to play. */
	#loading = false;
	/** The audio is not of the current track: it is on its way, or a takeover cut that short. */
	#stale = false;
	#volumeReport: ReturnType<typeof setTimeout> | undefined;
	/** This browser's other tabs, told when playback starts in this one. */
	#tabs = typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel('jewelcase.player');

	constructor() {
		realtime.on((message) => {
			if (message.type === 'hello') this.#progress.connected();
			else if (message.type === 'player') this.#heard(message.state);
			else if (message.type === 'execute') void this.#execute(message.command);
		});
		this.#tabs?.addEventListener('message', () => this.#yield());
	}

	/** The account's playback state, as the server just sent it. */
	#heard(state: PlayerState) {
		// Anything playing since, or a stop that is not this player's, as one heard on reconnecting.
		if (state.activeDeviceId !== null || state.trackId !== this.current?.id) {
			this.finished = false;
		}
		if (!realtime.deviceId) return;
		const action = this.#progress.heard(realtime.deviceId, state, this.playing);
		if (action === 'stop') this.#yield();
		else if (action === 'report') this.#report();
	}

	/** Carries out `command` from another device, and tells it how that went. */
	async #execute(command: PlayerCommand) {
		let reason: string | null;
		try {
			reason = await carryOut(this.#remote, command);
		} catch {
			reason = 'This device could not do that.';
		}
		const status = reason === null ? 'applied' : 'failed';
		realtime.send({
			type: 'commandResult',
			result: { commandId: command.commandId, status, reason: reason ?? undefined }
		});
	}

	/** Another device or tab took over: this one stops where it is, and can take it back. */
	#yield() {
		// Nor does a takeover from here it could not yet send.
		this.#progress.paused();
		// Nor does the lock screen show what no longer plays here (`requirements/playback.md` §7).
		if ('mediaSession' in navigator) navigator.mediaSession.metadata = null;
		if (!this.playing) return;
		// A track still loading must not start once its audio arrives.
		if (this.#loading) {
			this.#load++;
			this.#stale = true;
		}
		this.playing = this.#loading = false;
		this.#audio?.pause();
		this.#plays.end('stopped');
	}

	/**
	 * Takes the account's playback over here, playing, or paused if not `play`: the track it
	 * names, from where it got to, within the context this player has, if the track is in it, or
	 * on its own. Until the server keeps the queue, what came after it on the device it was on
	 * does not come along. The volume stays this device's own. Fails if there is nothing to pick
	 * up, or playback moves on meanwhile.
	 */
	async pickUp(play = true) {
		const state = playback.state;
		const { libraryId, trackId } = state ?? {};
		if (!libraryId || libraryId !== this.libraryId || !trackId) {
			throw new Error('Nothing here to pick up.');
		}
		const track =
			playback.track?.id === trackId ? playback.track : await getTrack(fetch, libraryId, trackId);
		// Another device took over, or this one changed library, while the track was on its way.
		const now = playback.state;
		const moved = now?.trackId !== trackId || now.activeDeviceId !== state?.activeDeviceId;
		if (moved || libraryId !== this.libraryId) throw new Error('Playback moved on.');
		const seconds = Math.min(playback.positionAt(Date.now()), track.durationUs / 1_000_000);
		let index = this.tracks.findIndex((t) => t.id === track.id);
		if (index < 0) {
			this.tracks = [track];
			this.context = { type: 'library' };
			index = 0;
		}
		this.#started();
		this.#go(index, seconds, play);
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
		if (!this.#audio || !this.current) return;
		if (this.playing) this.pause();
		else void this.resume().catch(() => {});
	}

	/** Plays the current track from where it is: settles once the audio plays, or fails. */
	async resume() {
		await this.#resume(true);
	}

	/** Resumes, as the user here (`byUser`) or another device would. */
	async #resume(byUser: boolean) {
		const audio = this.#audio;
		if (!audio || !this.current) throw new Error('Nothing is loaded.');
		// The track is on its way, and its audio is not yet the audio element's.
		if (this.#loading) return;
		if (byUser) this.#started();
		if (this.#stale) this.#go(this.index, this.position);
		else {
			// Shown again, as yielding to another tab or device clears it.
			this.#mediaSession(this.current);
			await audio.play();
		}
	}

	pause() {
		this.#progress.paused();
		// A track still on its way stays paused once its audio arrives.
		this.playing = this.#loading = false;
		this.#audio?.pause();
	}

	next() {
		if (nextPlayable(this.tracks, this.index, 1) !== null) this.#started();
		this.#next();
	}

	previous() {
		this.#previous(true);
	}

	/** Back a track, or to the start of this one, as the user here (`byUser`) or another device would. */
	#previous(byUser: boolean) {
		const target = previousTarget(this.tracks, this.index, this.position);
		// Back to the start of this track takes nothing over; it leaves it playing or not.
		if (target.restart) this.seek(0);
		else {
			if (byUser) this.#started();
			this.#go(target.index);
		}
	}

	/**
	 * The player as another device controls it: as the user here would, but taking nothing
	 * over, since commands other than a volume or a transfer naming this device go only to the
	 * connection playing, and a command still on its way when another device took over must not
	 * take playback back. A track loading counts as playing.
	 */
	#remote: Controls = {
		loaded: () => this.current !== null,
		playing: () => this.playing,
		resume: () => this.#resume(false),
		pause: () => this.pause(),
		next: () => this.#next(),
		previous: () => this.#previous(false),
		seek: (seconds) => this.seek(seconds),
		setVolume: (volume) => this.setVolume(volume),
		receive: (play) => this.pickUp(play)
	};

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
			this.playing = this.#loading = false;
			this.#audio?.pause();
			this.#plays.end('skipped');
			// Stopped at the start of the last track, so playing again, here or anywhere, plays it:
			// here, loaded again as a new play, since the audio may be of the track before.
			this.position = 0;
			this.#stale = true;
			this.finished = true;
			this.#report('stopped');
			return;
		}
		this.#go(i);
	}

	/** Tells the server where the player is: `status`, or as the audio element says. */
	#report(status?: Status) {
		const track = this.current;
		if (!track || !realtime.deviceId) return;
		status ??= !this.playing
			? 'paused'
			: this.#loading || this.#buffering
				? 'buffering'
				: 'playing';
		this.#progress.report({
			// Its place in the context, until the server keeps the queue.
			itemId: String(this.index),
			libraryId: this.libraryId,
			trackId: track.id,
			status,
			positionMs: Math.round(this.position * 1000),
			volume: this.volume
		});
	}

	/** Plays `index` from `startAt` seconds in, or holds it there paused unless `play`. */
	#go(index: number, startAt = 0, play = true) {
		this.index = index;
		const track = this.tracks[index];
		const load = ++this.#load;
		const audio = this.#ensureAudio();
		// One that played to its end has ended its play already.
		this.#plays.end('skipped');
		audio.pause();
		this.position = startAt;
		this.duration = track.durationUs / 1_000_000;
		this.waveform = null;
		this.#mediaSession(track);
		this.playing = this.#loading = play;
		// Until its audio is set, the audio element still has the track before.
		this.#stale = true;
		this.#report();

		const lib = this.libraryId;
		getPlaybackInfo(fetch, lib, track.id)
			.then((info) => {
				if (load !== this.#load) return;
				audio.src = audioUrl(lib, track.id, info.variant);
				this.#stale = false;
				// Where it was to start, or was moved to meanwhile.
				if (this.position > 0) {
					// Before the audio loads this is where it starts, in most browsers; the rest
					// are moved to where it is by then, once they know how long it is.
					audio.currentTime = this.position;
					const start = () => {
						const at = this.position;
						if (load === this.#load && audio.currentTime < at - 1) audio.currentTime = at;
					};
					audio.addEventListener('loadedmetadata', start, { once: true });
				}
				// Unless paused meanwhile.
				if (this.#loading) return audio.play();
			})
			.catch((error) => {
				if (load !== this.#load) return;
				// The browser would not play what this page did not start itself, or paused it by
				// itself, or it was to wait paused, or was paused meanwhile: either way it waits
				// here, rather than playing the next track.
				const waits =
					!this.#loading || error?.name === 'NotAllowedError' || error?.name === 'AbortError';
				this.playing = this.#loading = false;
				if (waits) this.#report();
				// A track that cannot play is skipped in place, without a message (`requirements/conventions.md` §6).
				else this.#next();
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
			// Resuming a play under way is the same play.
			if (this.#plays.active || !this.current) return;
			this.#plays.start(this.libraryId, this.current.id, this.context, audio.currentTime);
		});
		audio.addEventListener('waiting', () => {
			this.#buffering = true;
			this.#report();
		});
		audio.addEventListener('pause', () => {
			// Changing track pauses the one before, while the new one is to play.
			if (this.#loading) return;
			this.playing = false;
			if (!audio.ended) {
				this.#plays.paused();
				this.#report();
			}
		});
		setInterval(() => {
			if (this.playing) this.#report();
		}, REPORT_EVERY_MS);
		audio.addEventListener('timeupdate', () => {
			// Not the time of the track before, while this one is on its way, nor of this one
			// before it knows where it is.
			if (!this.#stale && audio.readyState) this.position = audio.currentTime;
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
			// One waiting paused waits here, rather than playing the next track.
			if (audio.src && !audio.paused) this.#next();
		});
		this.#audio = audio;

		if ('mediaSession' in navigator) {
			const ms = navigator.mediaSession;
			// While another device holds playback, nothing here takes it back, as a headset sending
			// play as it connects would: "Play here" does.
			const here = () => !playback.elsewhere;
			ms.setActionHandler('play', () => here() && void this.resume().catch(() => {}));
			ms.setActionHandler('pause', () => this.pause());
			ms.setActionHandler('previoustrack', () => here() && this.previous());
			ms.setActionHandler('nexttrack', () => here() && this.next());
			ms.setActionHandler(
				'seekto',
				(d) => here() && d.seekTime !== undefined && this.seek(d.seekTime)
			);
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
