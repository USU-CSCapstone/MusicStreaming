// Local playback: one audio element playing the account's queue, which the server keeps and
// `queue/` holds on this device. The player plays the queue's current item and moves on by
// editing the queue (`next`, `previous`), applied here at once, so every device sees where it
// is. A start from a view plays its clicked track before anything is answered. It reports its
// progress to the server and stops when another device takes over (`progress.ts`), carries out
// commands from the user's other devices while it plays, and takes playback over when one moves
// it here (`remote.ts`). It plays in one tab of this browser at a time, since every tab is the
// same device to the server.

import { untrack } from 'svelte';
import {
	ApiError,
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
import { firstPlayable, type PlayFrom, restartsOnPrevious } from './queue';
import { queue } from './queue/client.svelte';
import { realtime } from './realtime.svelte';
import { carryOut, type Controls } from './remote';
import { decodeWaveform } from './waveform';

/** How often a playing player says where it is, so a device picking up is never far behind. */
const REPORT_EVERY_MS = 10_000;
/** How long a track the server could not be reached for waits before it is asked for again. */
const RETRY_MS = 3_000;
/** How long the volume rests before it is reported, so dragging the slider is one report. */
const VOLUME_SETTLES_MS = 250;

type Status = 'playing' | 'paused' | 'stopped' | 'buffering';

/** A track and the library its audio is in. */
type Loaded = { track: TrackSummary; libraryId: string };

/** Whether a failure to load a track is the connection's or the server's, so that trying again may work. */
const isTransient = (error: unknown) =>
	error instanceof TypeError ||
	(error instanceof ApiError &&
		(error.status >= 500 || error.status === 429 || error.status === 408));

/** A random seed for a shuffle, which fixes its order. */
const newSeed = () => crypto.getRandomValues(new Uint32Array(1))[0];

class Player {
	/** The library the user browses, which a track picked up from another device must be of. */
	libraryId = $state('');

	/** The queue item loaded, or null. */
	get item(): string | null {
		return this.#item;
	}

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
	#item = $state<string | null>(null);
	/** What `current` is, with the library its audio is in. */
	#loaded: Loaded | null = null;
	/**
	 * Tracks by ID: what is playing and next, so moving on needs no request. The queue's rows
	 * name tracks; this holds what the player shows and plays of them.
	 */
	#summaries = new Map<string, TrackSummary>();
	/**
	 * Whether this device holds playback, playing or paused: it plays whatever the queue's
	 * current item becomes, whoever moved it. Other devices and tabs only show it.
	 */
	#holds = false;
	/** A start from a view is on its way to the queue, which still holds the one before. */
	#starting = false;
	/** What waits for the start is the passing over of this item, a track that cannot play. */
	#wantsSkip: string | null = null;
	/** Next was pressed while a start was on its way. */
	#wantsNext = false;
	/** The queue moved to an item that is not loaded yet, which is being looked up. */
	#moving = false;
	/** Which way the last move went, so a track that cannot play is passed over the same way. */
	#direction: 1 | -1 = 1;
	/** The items passed over in a row, so a run that comes round to one again, as under Repeat All, has tried the whole queue. */
	#tried = new Set<string>();
	/** Tries again a track the server could not be reached for. */
	#retryTimer: ReturnType<typeof setTimeout> | undefined;
	/** The track started from a view is the only one loaded, until the server's queue says which item it is. */
	#adoptable = false;
	/** Where a restored queue's current item resumes, until it is loaded. */
	#resumeAt: { item: string; seconds: number } | null = null;
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

	/** Listens to the realtime connection, and follows the queue while this device holds playback. */
	constructor() {
		realtime.on((message) => {
			if (message.type === 'hello') this.#progress.connected();
			else if (message.type === 'player') this.#heard(message.state);
			else if (message.type === 'execute') void this.#execute(message.command);
		});
		this.#tabs?.addEventListener('message', () => this.#yield());
		$effect.root(() => {
			$effect(() => {
				// Every change to the queue: an item still unknown may be known by now.
				const { current } = queue;
				void queue.revision;
				untrack(() => this.#follow(current));
			});
			$effect(() => {
				// The server has the item, or has moved on: what it was not told while it lacked the
				// item, or refused for that, is told now, once per change.
				const { held } = queue;
				void queue.version;
				untrack(() => this.#told(held));
			});
			$effect(() => {
				const { refusal } = queue;
				// A start the server refused: the queue is the old one again, and the track chosen
				// stays where it is, paused, rather than the audio switching to another queue's.
				if (refusal && refusal.edit === null) untrack(() => this.#refused(refusal.message));
			});
		});
	}

	/** Reports the loaded item if the server holds it and its state does not show this device at it. */
	#told(held: string | null) {
		if (held === null || held !== this.#item) return;
		const state = playback.state;
		if (state?.activeDeviceId === realtime.deviceId && state?.itemId === held) return;
		this.#report();
	}

	/** Pauses on the track a refused start was for, and says why. */
	#refused(message: string) {
		this.#holds = this.#wantsNext = this.#moving = false;
		this.pause();
		playback.fail(message);
	}

	/**
	 * Takes in the queue's current item. The device holding playback plays it, whoever moved
	 * it, unless a start of its own is still on its way.
	 */
	#follow(item: string | null) {
		if (item === null) this.#moving = false;
		if (!this.#holds || this.#starting || item === null || item === this.#item) return;
		void this.#open(item);
	}

	/**
	 * Plays `item`, the queue's current item, once its row is known (`design/queue.md` §3). A row
	 * still being learned is waited for: this runs again with each change to the queue. One that
	 * cannot be learned, or whose track cannot play, is passed over in place
	 * (`requirements/conventions.md` §6).
	 */
	async #open(item: string) {
		const open = ++this.#opens;
		this.#moving = true;
		let found: Loaded | 'skip' | 'wait' = 'wait';
		try {
			const { rows } = await queue.rows('current', 0, 1);
			const row = rows[0];
			// Not the queue's now: it moved again while the row was on its way.
			if (open !== this.#opens || row?.item !== item) return;
			if (row.status === 'unavailable') found = 'skip';
			else if (row.status === 'known') {
				const track = await this.#summary(row.libraryId, row.track!).catch((error) => {
					// A track that is gone or out of reach is passed over; one the network failed to
					// give waits, as one that cannot be loaded does.
					if (error instanceof ApiError && [403, 404].includes(error.status)) return null;
					throw error;
				});
				if (open !== this.#opens) return;
				found = track?.availability === 'available' ? { track, libraryId: row.libraryId } : 'skip';
			}
		} catch {
			// The server could not be reached: it waits, and asks again.
			if (open === this.#opens) {
				this.#moving = this.playing = this.#loading = false;
				this.#retry(() => this.#follow(queue.current));
			}
			return;
		}
		if (found === 'wait') return;
		if (found === 'skip') return this.#skip(item);
		this.#direction = 1;
		// The server's queue made the track the view clicked another item: the same track goes on.
		if (this.#adoptable && found.track.id === this.current?.id) {
			this.#adoptable = false;
			this.#moving = false;
			this.#item = item;
			this.#report();
			return;
		}
		const startAt = this.#resumeAt?.item === item ? this.#resumeAt.seconds : 0;
		this.#resumeAt = null;
		this.#go(found.track, found.libraryId, item, startAt);
		void this.#preload();
	}

	/** Runs `again` after a wait, in place of any other waiting. */
	#retry(again: () => void) {
		clearTimeout(this.#retryTimer);
		this.#retryTimer = setTimeout(again, RETRY_MS);
	}

	/**
	 * Passes over `item`, which cannot play, on the way the player was going: one more `next` or
	 * `previous`, which the queue applies at once. Back past the first of History it goes on
	 * forward. A run that comes round to an item it has passed over has tried the whole queue,
	 * under Repeat All too: playback stops and says that nothing in it can play.
	 */
	#skip(item: string) {
		// The queue still has the one before: moving on waits for the new one.
		if (this.#starting) {
			this.#wantsNext = true;
			this.#wantsSkip = item;
			return;
		}
		if (this.#tried.has(item)) {
			playback.fail('None of the songs in the queue can play.');
			return this.#end();
		}
		this.#tried.add(item);
		if (this.#direction === -1 && queue.counts.history > 0) {
			queue.edit({ type: 'previous', key: crypto.randomUUID() });
			return;
		}
		this.#direction = 1;
		if (this.#hasNext()) queue.edit({ type: 'next' });
		else this.#end();
	}

	/** The summary of the track `trackId` in `libraryId`, fetched unless held. */
	async #summary(libraryId: string, trackId: string): Promise<TrackSummary> {
		const held = this.#summaries.get(trackId);
		if (held) return held;
		const track = await getTrack(fetch, libraryId, trackId);
		this.#summaries.set(trackId, track);
		return track;
	}

	/** Fetches the summary of the item after the current one, and forgets the rest. */
	async #preload() {
		try {
			const {
				rows: [row]
			} = await queue.rows('upNext', 0, 1);
			if (row?.status === 'known') await this.#summary(row.libraryId, row.track!);
			for (const id of this.#summaries.keys()) {
				if (id !== this.current?.id && id !== row?.track) this.#summaries.delete(id);
			}
		} catch {
			// Fetched when it is reached.
		}
	}

	/** The account's playback state, as the server just sent it. */
	#heard(state: PlayerState) {
		// Anything playing since, or a stop that is not this player's, as one heard on reconnecting.
		if (state.activeDeviceId !== null || state.itemId !== this.#item) {
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
		this.#holds = this.#wantsNext = this.#moving = false;
		clearTimeout(this.#retryTimer);
		// Nor is a track still being looked up to be loaded.
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
	 * Takes the account's playback over here, playing, or paused if not `play`: the item the state
	 * names, from where it got to, which must be the queue's current item (playback moves
	 * without rebuilding the queue, `requirements/queue.md` §7). It fails, and starts nothing,
	 * if the queue here is not that far yet, or the item is of another library than this
	 * device's. The volume stays this device's own. Fails too if there is nothing to pick up, or
	 * playback moves on meanwhile.
	 */
	async pickUp(play = true) {
		const state = playback.state;
		const itemId = state?.itemId;
		if (!itemId) throw new Error('Nothing here to pick up.');
		if (itemId !== queue.current) throw new Error('The queue is not here yet.');
		const {
			rows: [row]
		} = await queue.rows('current', 0, 1);
		if (row?.item !== itemId || row.status !== 'known')
			throw new Error('The queue is not here yet.');
		if (row.libraryId !== this.libraryId) throw new Error('Nothing here to pick up.');
		const { libraryId, track: trackId } = row;
		const track =
			playback.track?.id === trackId ? playback.track : await getTrack(fetch, libraryId, trackId!);
		// Another device took over, or this one changed library, while the track was on its way.
		const now = playback.state;
		const moved = now?.itemId !== itemId || now.activeDeviceId !== state?.activeDeviceId;
		if (moved || libraryId !== this.libraryId) throw new Error('Playback moved on.');
		const seconds = Math.min(playback.positionAt(Date.now()), track.durationUs / 1_000_000);
		this.#summaries.set(track.id, track);
		this.#started();
		this.#go(track, libraryId, itemId, seconds, play);
		void this.#preload();
	}

	/**
	 * Starts a queue and plays `tracks[index]`, or the first track from there that can. The
	 * track plays at once; the queue is started here and sent to the server in the background.
	 * A view that pages through a list of `total` tracks passes its `from`, a recipe for the
	 * server to resolve, and its `tracks` are the rows it loaded; one with no recipe sends its
	 * tracks.
	 */
	play(
		tracks: TrackSummary[],
		index: number,
		context: PlayContext = { type: 'library' },
		from?: PlayFrom
	) {
		const at = firstPlayable(tracks, index);
		if (at === null) return;
		const libraryId = this.libraryId;
		const track = tracks[at];
		const key = crypto.randomUUID();
		const ids = tracks.map((t) => t.id);
		this.#summaries.set(track.id, track);
		this.#starting = true;
		this.#wantsNext = false;
		this.#wantsSkip = null;
		this.#started();
		this.#go(track, libraryId, `${key}.${at}`);
		this.#adoptable = true;
		this.#tried.clear();
		const source = from ? { key, libraryId, recipe: from.recipe } : { key, libraryId, tracks: ids };
		const request = from
			? { start: at, startTrackId: track.id, total: from.total, loaded: ids }
			: { start: at };
		void queue.start({ queueId: crypto.randomUUID(), context, source, ...request }).then(() => {
			this.#starting = false;
			void this.#preload();
			this.#carryOutWait();
		});
	}

	/** Carries out the Next or skip that came while a start or restore was on its way, now that it has settled. */
	#carryOutWait() {
		if (!this.#wantsNext) return;
		const skip = this.#wantsSkip;
		this.#wantsNext = false;
		this.#wantsSkip = null;
		if (skip) this.#skip(skip);
		else this.#advance();
	}

	/**
	 * Restores the kept queue `queueId` and plays it here from where it was left, taking playback
	 * over as a start does (`requirements/queue.md` §6). Nothing changes until the server has
	 * answered, and the first report, naming an item it holds, goes after. Settles with why it
	 * was not restored, or null.
	 */
	async restore(queueId: string): Promise<string | null> {
		// As a start's wait: the queue is not followed while the server's answer is on its way.
		this.#starting = true;
		const result = await queue.restore(queueId);
		this.#starting = false;
		if (!result.ok) {
			// What was pressed meanwhile applies to the queue that is still playing.
			this.#carryOutWait();
			return result.message;
		}
		this.#wantsNext = false;
		this.#wantsSkip = null;
		const item = queue.current;
		if (item === null) {
			// A queue played to its end has nothing to play, and the old audio is not its.
			this.#end();
			return null;
		}
		const { position } = result;
		this.#resumeAt = {
			item,
			seconds: position?.item === item ? position.positionMs / 1000 : 0
		};
		this.#adoptable = false;
		this.#tried.clear();
		this.#started();
		// As a jump: the restored item is followed, however long its row takes to be known.
		this.#holds = true;
		void this.#open(item);
		return null;
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
			this.#go(this.#loaded.track, this.#loaded.libraryId, this.#item, this.position);
		} else {
			// Shown again, as yielding to another tab or device clears it.
			this.#mediaSession(this.current);
			await audio.play();
		}
	}

	/** Pauses here, and stops waiting to ask again for a track the server could not be reached for. */
	pause() {
		// A pause is the user's word: a track waiting to be asked for again is not.
		clearTimeout(this.#retryTimer);
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
		if (restartsOnPrevious(this.position, queue.counts.history)) {
			this.seek(0);
			return;
		}
		if (byUser) this.#started();
		this.#direction = -1;
		this.#tried.clear();
		this.#adoptable = false;
		queue.edit({ type: 'previous', key: crypto.randomUUID() });
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

	/** The user picks `item` of Up Next: the queue jumps to it, and playback is taken over here to play it. */
	jump(item: string) {
		this.#started();
		this.#holds = true;
		this.#direction = 1;
		this.#tried.clear();
		this.#adoptable = false;
		queue.edit({ type: 'jump', item });
	}

	/**
	 * The user picks `play` of History: the queue goes back to it as Previous would that many
	 * times, and playback is taken over here to play it. The server refuses a play that is no
	 * longer there, and the queue client's `refusal` says so.
	 */
	rewind(play: { play: number; item: string }) {
		this.#started();
		this.#holds = true;
		this.#direction = -1;
		this.#tried.clear();
		this.#adoptable = false;
		queue.edit({ type: 'rewind', play, key: crypto.randomUUID() });
	}

	/** Turns shuffle on or off for the whole account's queue, with a new seed. */
	setShuffle(on: boolean) {
		queue.edit({ type: 'shuffle', on, seed: newSeed() });
	}

	/** Sets how the queue repeats. */
	setRepeat(mode: 'off' | 'all' | 'one') {
		queue.edit({ type: 'repeat', mode });
	}

	/** The user started or moved playback here, which takes it over from anywhere else. */
	#started() {
		this.#progress.started();
		this.#tabs?.postMessage('started');
	}

	/** Whether the queue has anything to play after the current item, or starts over. */
	#hasNext(): boolean {
		return queue.counts.upNext > 0 || queue.repeat === 'all';
	}

	/** On to the next item, which the queue moves to at once, or stopped at the end of the queue rather than wrapping. */
	#advance() {
		// The queue still has the one before.
		if (this.#starting) {
			this.#wantsNext = true;
			return;
		}
		if (!this.#hasNext()) return this.#end();
		this.#direction = 1;
		this.#tried.clear();
		this.#adoptable = false;
		queue.edit({ type: 'next' });
	}

	/** Stops at the end of the queue. */
	#end() {
		clearTimeout(this.#retryTimer);
		this.#holds = this.#moving = false;
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
		// The server refuses an item its queue lacks: a start's, or a Previous's copy, waits for it.
		if (!track || !this.#item || !this.#loaded || !realtime.deviceId) return;
		if (queue.held !== this.#item) return;
		status ??= !this.playing
			? 'paused'
			: this.#loading || this.#buffering
				? 'buffering'
				: 'playing';
		this.#progress.report({
			itemId: this.#item,
			status,
			positionMs: Math.round(this.position * 1000),
			volume: this.volume
		});
	}

	/** Plays `track`, the queue's `item`, from `startAt` seconds in, or holds it there paused unless `play`. */
	#go(track: TrackSummary, libraryId: string, item: string, startAt = 0, play = true) {
		clearTimeout(this.#retryTimer);
		this.#loaded = { track, libraryId };
		this.#holds = true;
		this.#moving = false;
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

		const lib = libraryId;
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
				const retries = !waits && isTransient(error);
				this.playing = this.#loading = false;
				if (waits) this.#report();
				// The connection failed, not the track: it waits and is asked for again, never skipped.
				else if (retries) {
					this.#report();
					this.#retry(() => {
						if (this.#holds && this.#item === item) this.#go(track, libraryId, item, this.position);
					});
				}
				// A track that cannot play is skipped in place, without a message (`requirements/conventions.md` §6).
				else this.#skip(item);
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
			// Something plays: a run of tracks that would not has ended.
			this.#tried.clear();
			this.#buffering = false;
			this.#loading = false;
			this.#report();
			// Resuming a play under way is the same play.
			if (this.#plays.active || !this.current || !this.#loaded) return;
			this.#plays.start(
				this.#loaded.libraryId,
				this.current.id,
				queue.context ?? { type: 'library' },
				queue.playingManual ? 'manual' : 'context',
				audio.currentTime
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
			// A move under way is waiting for the item it goes to.
			if (this.#moving) return;
			// Repeat One is this device's to carry out; Repeat All is the queue's.
			if (queue.repeat === 'one' && this.#loaded && this.#item) {
				this.#go(this.#loaded.track, this.#loaded.libraryId, this.#item);
			} else this.#advance();
		});
		// Closing the page stops what is playing. The report must outlive the page.
		addEventListener('pagehide', () => this.#plays.end('stopped', true));
		audio.addEventListener('error', () => {
			// One waiting paused waits here, rather than playing the next track.
			if (audio.src && !audio.paused && this.#item) this.#skip(this.#item);
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
