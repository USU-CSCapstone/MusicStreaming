// Local playback: one audio element playing the account's queue, which the server keeps and
// `queue/` holds on this device. The player plays the queue's current item and moves on by
// editing the queue (`next`, `previous`, `jump`), so every device sees where it is. It reports
// its progress to the server and stops when another device takes over (`progress.ts`), carries
// out commands from the user's other devices while it plays, and takes playback over when one
// moves it here (`remote.ts`). It plays in one tab of this browser at a time, since every tab
// is the same device to the server.

import { untrack } from 'svelte';
import {
	audioUrl,
	getPlaybackInfo,
	getTracks,
	getWaveform,
	imageUrl,
	recordPlays
} from './api/client';
import type {
	PlayContext,
	PlayerCommand,
	PlayerState,
	QueueOrigin,
	QueueQuery,
	TrackSummary
} from './api/types';
import { albumTitle, artistName } from './format';
import { playback } from './playback.svelte';
import { PlayRecorder } from './plays';
import { Progress } from './progress';
import { queue } from './queue/client.svelte';
import { firstPlayable, restartsOnPrevious } from './queue';
import { realtime } from './realtime.svelte';
import { carryOut, type Controls } from './remote';
import { decodeWaveform } from './waveform';

/** How often a playing player says where it is, so a device picking up is never far behind. */
const REPORT_EVERY_MS = 10_000;
/** How long the volume rests before it is reported, so dragging the slider is one report. */
const VOLUME_SETTLES_MS = 250;
/** How many queue items are looked at together when looking for one that can play. */
const LOOK_AHEAD = 100;

type Status = 'playing' | 'paused' | 'stopped' | 'buffering';

/** A track, with the library it is in. */
type Found = { track: TrackSummary; libraryId: string };

class Player {
	libraryId = '';

	/** The track loaded, playing or waiting where it stopped. */
	current = $state<TrackSummary | null>(null);
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

	/** The queue item `current` is, which reports name. */
	#item: string | null = null;
	/** What `current` is, with its library, which its audio comes from. */
	#loaded: Found | null = null;
	/** Whether the item being moved to came off the Manual Queue, which plays record. */
	#origin: QueueOrigin = 'context';
	/**
	 * Tracks by ID: what is playing and next, and what a start or a look-ahead just fetched, so
	 * moving on needs no request.
	 */
	#summaries = new Map<string, Found>();
	/** Set while this player waits for the queue to move where it asked, to the way it went. */
	#awaiting: 1 | -1 | null = null;
	/**
	 * Whether this device holds playback, playing or paused: it plays whatever the queue's current
	 * item becomes, whoever moved it. Other devices and tabs only show it.
	 */
	#holds = false;
	/** Next was pressed, or a track ended, while a start from a list had only its first track. */
	#wantsNext = false;
	/** The item a run of `previous` edits leads to; the items passed on the way are not opened. */
	#backTo: string | null = null;
	/**
	 * A start from a list is under way and its track not yet loaded: the server's queue, whose
	 * items are not the one-track placeholder's, is opened like the placeholder was.
	 */
	#starting = false;
	/** Under Repeat All, a restart was asked for because nothing after the current item plays. */
	#wrapped = false;
	/** The items such a run passes through, the last of them `#backTo`. */
	#passing: string[] = [];
	/** Bumped on every item opened, so a slow lookup for an earlier one is ignored. */
	#opens = 0;
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
		$effect.root(() => {
			$effect(() => {
				const { current, queueId, settling } = queue;
				untrack(() => this.#follow(current, queueId !== null, settling));
			});
		});
	}

	/**
	 * Takes in the queue's current item `item`. What this player asked for is opened and played.
	 * Any other move is followed only by the device holding playback; a queue that is gone, as
	 * when the server has none, stops it.
	 */
	#follow(item: string | null, hasQueue: boolean, settling: boolean) {
		if (!hasQueue) {
			if (this.#item) this.#release();
			return;
		}
		if (this.#wantsNext && !settling) {
			this.#wantsNext = false;
			void this.#advanceFrom(item);
			return;
		}
		if (item === this.#item) return;
		let direction = this.#awaiting;
		if (direction === null && this.#starting && !settling) direction = 1;
		if (direction !== null && this.#backTo !== null) {
			// Stepping back: only the item the run lands on is opened, and one it does not pass
			// through is a change made elsewhere, which is followed like any other.
			if (item !== null && this.#passing.includes(item)) {
				if (item !== this.#backTo) return;
			} else {
				this.#awaiting = this.#backTo = direction = null;
			}
		}
		if (direction === null) {
			if (item && this.#holds && !playback.elsewhere) void this.#adopt(item);
			return;
		}
		this.#awaiting = null;
		this.#backTo = null;
		if (item === null) return this.#end();
		void this.#open(item, direction);
	}

	/**
	 * Moves on, as Next was wanted before the server's queue arrived: `item`, if it is the track
	 * already loaded, is first taken up as the current one, so the move is followed like any other.
	 */
	async #advanceFrom(item: string | null) {
		const ref = item ? await queue.track(item).catch(() => null) : null;
		if (ref?.trackId === this.current?.id) this.#item = item;
		this.#advance();
	}

	/**
	 * Takes up an item the queue moved to that this device did not choose: the same track as
	 * the one loaded, as the server's queue is for a start from a list, only changes the item
	 * reports name; any other is opened, playing or paused as this is.
	 */
	async #adopt(item: string) {
		const ref = await queue.track(item).catch(() => null);
		if (!ref || queue.current !== item) return;
		if (ref.trackId === this.current?.id) this.#item = item;
		else {
			this.#origin = 'context';
			void this.#open(item, 1, this.playing);
		}
	}

	/** Stops and shows nothing, as when the queue is gone. */
	#release() {
		this.#item = null;
		this.#holds = this.#wantsNext = this.#wrapped = this.#starting = false;
		this.#awaiting = this.#backTo = null;
		this.#opens++;
		this.#load++;
		this.playing = this.#loading = false;
		this.#audio?.pause();
		this.#plays.end('stopped');
		this.current = this.#loaded = null;
		this.waveform = null;
	}

	/**
	 * Plays `item`, the queue's current item, travelling `direction`, or holds it paused unless
	 * `play`. If its track cannot play it is passed over in place (`requirements/conventions.md`
	 * §6): with one `jump` to the nearest after it that can play, or a stop at the end.
	 */
	async #open(item: string, direction: 1 | -1, play = true) {
		const open = ++this.#opens;
		try {
			const [found] = await this.#lookup([item]);
			// Not the queue's now: another start or device moved it while the track was on its way.
			if (open !== this.#opens || queue.current !== item) return;
			if (found?.track.availability === 'available') {
				this.#go(found, item, 0, play);
				void this.#preload();
				return;
			}
			const target = await this.#playable();
			if (open !== this.#opens) return;
			this.#awaiting = 1;
			if (target === null) {
				// Repeat All starts the context over, once: a lap with nothing playable ends it.
				if (queue.repeat !== 'all' || this.#wrapped) {
					this.#awaiting = null;
					return this.#end();
				}
				this.#wrapped = true;
				this.#origin = 'context';
				queue.edit({ type: 'next' });
				return;
			}
			this.#origin = target.origin;
			queue.edit({ type: 'jump', item: target.item });
		} catch {
			// The track's details could not be had: it waits, as a track that cannot be loaded does.
			if (open === this.#opens) this.playing = this.#loading = false;
		}
	}

	/** The nearest item after the current one whose track can play, with the list it is in, or null. */
	async #playable(): Promise<{ item: string; origin: QueueOrigin } | null> {
		for (const [list, origin] of [
			['manual', 'manual'],
			['upcoming', 'context']
		] as const) {
			for (let at = 0; ; at += LOOK_AHEAD) {
				const items = await queue.window(list, at, LOOK_AHEAD);
				if (items.length === 0) break;
				const found = await this.#lookup(items);
				const hit = items.find((_, i) => found[i]?.track.availability === 'available');
				if (hit) return { item: hit, origin };
			}
		}
		return null;
	}

	/** Fetches the summary of the item after the current one, and forgets the rest. */
	async #preload() {
		try {
			const next =
				(await queue.window('manual', 0, 1))[0] ?? (await queue.window('upcoming', 0, 1))[0];
			const [found] = next ? await this.#lookup([next]) : [];
			for (const id of this.#summaries.keys()) {
				if (id !== this.current?.id && id !== found?.track.id) this.#summaries.delete(id);
			}
		} catch {
			// Fetched when it is reached.
		}
	}

	/**
	 * The tracks `items` play, with their libraries, in the same order. One out of reach, or that the queue does not
	 * know, is null. Up to `LOOK_AHEAD` items go in one request.
	 */
	async #lookup(items: string[]): Promise<(Found | null)[]> {
		const refs = await Promise.all(items.map((item) => queue.track(item)));
		const wanted: Record<string, string[]> = {};
		for (const ref of refs) {
			if (ref && !this.#summaries.has(ref.trackId))
				(wanted[ref.libraryId] ??= []).push(ref.trackId);
		}
		await Promise.all(
			Object.entries(wanted).map(async ([library, ids]) => {
				for (const track of (await getTracks(fetch, library, [...new Set(ids)])).items) {
					this.#summaries.set(track.id, { track, libraryId: library });
				}
			})
		);
		return refs.map((ref) => (ref && this.#summaries.get(ref.trackId)) || null);
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
		this.#holds = this.#wantsNext = false;
		// Nor is a track still being looked up to be loaded.
		this.#awaiting = null;
		this.#opens++;
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
	 * Takes the account's playback over here, playing, or paused if not `play`: the item it
	 * names, which the queue this device holds must have, from where it got to. The volume stays
	 * this device's own. Fails if there is nothing to pick up, or playback moves on meanwhile.
	 */
	async pickUp(play = true) {
		const state = playback.state;
		const { libraryId, trackId, itemId } = state ?? {};
		if (!libraryId || libraryId !== this.libraryId || !trackId || !itemId) {
			throw new Error('Nothing here to pick up.');
		}
		if (playback.track?.id === trackId) {
			this.#summaries.set(trackId, { track: playback.track, libraryId });
		}
		// An item of another queue is not known here.
		const [found] = await this.#lookup([itemId]);
		if (!found) throw new Error('Nothing here to pick up.');
		// Another device took over, or this one changed library, while the track was on its way.
		const now = playback.state;
		const moved = now?.trackId !== trackId || now.activeDeviceId !== state?.activeDeviceId;
		if (moved || libraryId !== this.libraryId) throw new Error('Playback moved on.');
		const seconds = Math.min(playback.positionAt(Date.now()), found.track.durationUs / 1_000_000);
		this.#started();
		this.#awaiting = null;
		this.#origin = 'context';
		this.#go(found, itemId, seconds, play);
		// The queue is the account's: playing the item here leaves it where it was, unless it had moved.
		if (queue.current !== itemId) queue.edit({ type: 'jump', item: itemId });
		void this.#preload();
	}

	/**
	 * Starts a queue from `context` and plays `tracks[index]`, or the first track from there that
	 * can. A view that holds all its tracks sends them; one that pages through a list passes its
	 * `query`, for the server to read to the end, and the queue is then what the server made of it.
	 */
	play(
		tracks: TrackSummary[],
		index: number,
		context: PlayContext = { type: 'library' },
		query?: QueueQuery
	) {
		const start = firstPlayable(tracks, index);
		if (start === null) return;
		// The track the user chose is known, and plays before the queue says so.
		const libraryId = this.libraryId;
		this.#summaries.set(tracks[start].id, { track: tracks[start], libraryId });
		this.#started();
		this.#awaiting = 1;
		this.#wantsNext = false;
		this.#starting = query !== undefined;
		this.#origin = 'context';
		queue.start(
			query
				? { context, libraryId, query, startTrackId: tracks[start].id }
				: { context, libraryId, tracks: tracks.map((track) => track.id), start }
		);
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
		if (this.#stale && this.#item && this.#loaded) {
			this.#go(this.#loaded, this.#item, this.position);
		} else {
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

	/** The user's Next: moves the queue on, taking playback over here, or stops at its end. */
	next() {
		if (this.#hasNext()) this.#started();
		this.#advance();
	}

	/** The user's Previous: restarts the track if it is under way or first, else steps back. */
	previous() {
		this.#previous(true);
	}

	/** Back a track, or to the start of this one, as the user here (`byUser`) or another device would. */
	#previous(byUser: boolean) {
		// Back to the start of this track takes nothing over; it leaves it playing or not.
		if (restartsOnPrevious(this.position, queue.counts.history)) this.seek(0);
		else void this.#stepBack(byUser);
	}

	/**
	 * Steps back with `previous` edits to the nearest item in History that can play, so what is
	 * passed over returns to Up Next in order. With none, the track restarts and nothing changes.
	 */
	async #stepBack(byUser: boolean) {
		let back: { steps: number; item: string; passing: string[] } | null;
		try {
			back = await this.#stepsBack();
		} catch {
			back = null;
		}
		if (back === null) return this.seek(0);
		if (byUser) this.#started();
		this.#awaiting = -1;
		this.#origin = 'context';
		this.#backTo = back.item;
		this.#passing = back.passing;
		for (let i = 0; i < back.steps; i++) queue.edit({ type: 'previous' });
	}

	/** The nearest item in History that can play, and how many `previous` edits reach it, or null. */
	async #stepsBack(): Promise<{ steps: number; item: string; passing: string[] } | null> {
		const passed: string[] = [];
		for (let end = queue.counts.history; end > 0; end -= LOOK_AHEAD) {
			const at = Math.max(0, end - LOOK_AHEAD);
			const items = (await queue.window('history', at, end - at)).toReversed();
			const found = await this.#lookup(items);
			const hit = found.findIndex((one) => one?.track.availability === 'available');
			if (hit >= 0) {
				const passing = [...passed, ...items.slice(0, hit + 1)];
				return { steps: passing.length, item: items[hit], passing };
			}
			passed.push(...items);
		}
		return null;
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
		next: () => this.#advance(),
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

	/** Whether the queue has anything to play after the current item, or starts over. */
	#hasNext(): boolean {
		const { manual, upcoming } = queue.counts;
		return manual + upcoming > 0 || queue.repeat === 'all' || queue.settling;
	}

	/** On to the next item, or stopped at the end of the queue rather than wrapping. */
	#advance() {
		if (!this.#hasNext()) return this.#end();
		// A start from a list has only its first track until the server's queue arrives.
		if (queue.settling) {
			this.#wantsNext = true;
			return;
		}
		this.#awaiting = 1;
		this.#origin = queue.counts.manual > 0 ? 'manual' : 'context';
		queue.edit({ type: 'next' });
	}

	/** Stops at the end of the queue. */
	#end() {
		this.#wrapped = this.#starting = false;
		this.playing = this.#loading = false;
		this.#audio?.pause();
		this.#plays.end('skipped');
		// Stopped at the start of the last track, so playing again, here or anywhere, plays it:
		// here, loaded again as a new play, since the audio may be of the track before.
		this.position = 0;
		this.#stale = true;
		this.finished = true;
		this.#report('stopped');
	}

	/** Tells the server where the player is: `status`, or as the audio element says. */
	#report(status?: Status) {
		const track = this.current;
		if (!track || !this.#item || !realtime.deviceId) return;
		status ??= !this.playing
			? 'paused'
			: this.#loading || this.#buffering
				? 'buffering'
				: 'playing';
		this.#progress.report({
			itemId: this.#item,
			libraryId: this.#loaded!.libraryId,
			trackId: track.id,
			status,
			positionMs: Math.round(this.position * 1000),
			volume: this.volume
		});
	}

	/** Plays `track`, the queue's `item`, from `startAt` seconds in, or holds it there paused unless `play`. */
	#go(found: Found, item: string, startAt = 0, play = true) {
		const { track } = found;
		this.#wrapped = this.#starting = false;
		this.#loaded = found;
		this.#holds = true;
		this.current = track;
		this.#item = item;
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

		const lib = found.libraryId;
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
				else this.#advance();
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
			// The audio is still the track before's, which began playing after it was left.
			if (this.#stale) return;
			this.#buffering = false;
			this.#loading = false;
			this.#report();
			// Resuming a play under way is the same play.
			if (this.#plays.active || !this.current) return;
			this.#plays.start(
				this.#loaded!.libraryId,
				this.current.id,
				queue.context ?? { type: 'library' },
				audio.currentTime,
				this.#origin
			);
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
			// The audio of a track already left, or a move under way, is not this track ending.
			if (this.#stale || this.#awaiting !== null) return;
			// Repeat One is this device's to carry out; Repeat All is the queue's.
			if (queue.repeat === 'one' && this.#loaded && this.#item) this.#go(this.#loaded, this.#item);
			else this.#advance();
		});
		// Closing the page stops what is playing. The report must outlive the page.
		addEventListener('pagehide', () => this.#plays.end('stopped', true));
		audio.addEventListener('error', () => {
			// One waiting paused waits here, rather than playing the next track.
			if (audio.src && !audio.paused) this.#advance();
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
				? [
						{
							src: imageUrl(this.#loaded?.libraryId ?? this.libraryId, image, 512),
							sizes: '512x512',
							type: 'image/jpeg'
						}
					]
				: []
		});
	}
}

export const player = new Player();
