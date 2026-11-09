// The page's side of the catalog: one tab at a time owns the catalog and runs its worker
// (`worker.ts`); every tab's `query` and `signOut` reach the owner, through `router.ts` when the
// tab is not it. The owner tells the worker who is signed in and what the realtime connection
// hears, and shows what the worker cannot fix itself.

import { API } from '$lib/api/root';
import { connectivity } from '$lib/connectivity.svelte';
import { clearImageCache } from '$lib/image-cache';
import { clearQueueLists } from '$lib/queue/lists';
import { forget } from '$lib/last-known';
import wasmUrl from '$lib/catalog-wasm/jewelcase_catalog_wasm_bg.wasm?url';
import type { RealtimeServerMessage } from '$lib/api/types';
import type { Realtime } from '$lib/realtime.svelte';
import { toast } from '$lib/toast.svelte';
import type { Ask, FromWorker, Request, ToWorker } from './protocol';
import { CHANNEL, CatalogRouter, TIMEOUT_MS, type Turn } from './router';

/** The realtime messages that make the worker pull. */
const PULLING = new Set(['hello', 'libraryChanged', 'accountChanged']);
/** How long the owner waits for its worker to release the pool before stopping it anyway. */
const PAUSE_MS = 3000;
/** Tries at a sign-out another tab's owner has to answer. */
const SIGN_OUT_TRIES = 3;
/** How long a sign-out waits for another tab's owner before trying again. */
const SIGN_OUT_MS = 3000;
/** How long the owner waits for its worker to open the files before giving the lock up. */
const START_MS = 20_000;
/** How long a read waits to learn where a library stands before the server answers it instead. */
const STATE_MS = 1500;
/** How long a read waits for this tab's own election and worker, when it is about to own the catalog. */
const OWNER_MS = 5000;
/** How long a tab that heard no answer from an owner online goes to the server without asking it again. */
const DEAF_MS = 30_000;
/** How often that wait looks for the lock. */
const LOOK_MS = 25;

/** Whether the browser can elect an owner at all; without Web Locks (plain HTTP, old iOS) the catalog is unavailable. */
const available = () => !!navigator.locks;

/** Starts the catalog worker. */
const spawn = () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });

/** The page's handle on the catalog, whichever tab owns it. */
export class CatalogClient {
	#spawn: () => Worker;
	#name: string;
	/** Whether this tab is out of sight; tests give each tab its own. */
	#hidden: () => boolean;
	#router: CatalogRouter | null = null;
	#worker: Worker | null = null;
	#user = '';
	#unsubscribe: (() => void) | null = null;
	#persisted = false;
	#listening = false;
	/** This tab is in the election, or about to be: it will own the catalog or another tab will. */
	#wants = false;
	/** The device is working offline, which a worker is told as it starts and when it changes. */
	#workingOffline = connectivity.workOffline;
	/** The sign-out under way, which this tab's next election waits for. */
	#signingOut: Promise<unknown> = Promise.resolve();
	/** This tab holds the lock, whether or not its worker has started. */
	#holding = false;
	/** The page is about to freeze: the owner stops its worker without waiting for it. */
	#hurry = false;
	/** This tab answers requests as the owner. */
	#serving = false;
	/** Until when an online read goes to the server without asking an owner that did not answer. */
	#deafUntil = 0;
	/** Ends this tab's wait for the lock, or its hold of it. */
	#abort: AbortController | null = null;
	#release: (() => void) | null = null;
	/** Settles once this tab's latest request for the lock is over. */
	#held: Promise<unknown> = Promise.resolve();
	/** What waits for the worker's answer to a request, by the request's ID. */
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- waiting askers are not state to render
	#calls = new Map<number, { resolve: (json: string) => void; reject: (error: Error) => void }>();
	#callId = 0;
	/** What waits for a `started` or `paused` from the worker. */
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- waiting askers are not state to render
	#says = new Map<string, () => void>();

	/** Makes a client whose owner starts its worker with `create`, on the lock and channel `name`; tests pass their own, and `hidden`. */
	constructor(create: () => Worker = spawn, name = CHANNEL, hidden = () => document.hidden) {
		this.#spawn = create;
		this.#name = name;
		this.#hidden = hidden;
	}

	/**
	 * Takes part in electing the tab that owns the catalog for `user`, and feeds the realtime
	 * messages that make it pull to the worker if this tab wins, until
	 * [`disconnect`](#disconnect).
	 */
	connect(realtime: Pick<Realtime, 'on'>, user: string) {
		// Already in the election for this account, from `preconnect`: it carries on.
		const electing = this.#wants && this.#user === user;
		if (!electing) this.disconnect();
		this.#unsubscribe?.();
		this.#unsubscribe = realtime.on((message: RealtimeServerMessage) => {
			if (PULLING.has(message.type)) this.#send({ type: 'message', message });
		});
		if (!electing) this.#begin(user);
		// A tab that started out of view and was shown before this is still out of the queue.
		else this.#resume();
	}

	/**
	 * Joins the election for `user` before the app has loaded its first page, so a page that
	 * loads at once, as an app launched offline does, finds an owner coming. Without
	 * [`connect`](#connect) it hears no realtime messages.
	 */
	preconnect(user: string) {
		if (!this.#wants) this.#begin(user);
	}

	/** Enters the election for `user`. */
	#begin(user: string) {
		this.#wants = true;
		this.#persist();
		this.#user = user;
		if (!this.#listening) {
			this.#listening = true;
			// A page that is going away or frozen gives the catalog up, so a tab in front can take it.
			addEventListener('pagehide', () => this.#yield());
			addEventListener('pageshow', (event) => event.persisted && this.#resume());
			// Chromium freezes a hidden page without `pagehide`, and a frozen owner must not keep the
			// lock: its worker is stopped without the pause round trip, which needs a task the
			// frozen page never runs, so the lock goes with the microtasks that follow.
			document.addEventListener('freeze', () => {
				this.#hurry = true;
				this.#yield();
			});
			document.addEventListener('resume', () => this.#resume());
			// No browser fires `pagehide` for a tab switch, and a hidden tab waiting for the catalog is
			// no reason to move it: an owner out of sight hands over only to a waiting tab in view.
			document.addEventListener('visibilitychange', () => this.#visibility());
		}
		// A tab hearing a sign-out leaves, so a signed-out tab never owns the next account's files.
		this.#route();
		if (available()) this.#join();
	}

	/**
	 * Joins the election once a sign-out under way has finished, so the next owner is never wiped
	 * by it, and only if the tab is still connected and not already in it.
	 */
	#join() {
		const connection = this.#unsubscribe;
		void this.#signingOut.then(
			() => this.#unsubscribe === connection && !this.#abort && this.#elect()
		);
	}

	/** Tells the worker, if this tab owns one, whether the device is working offline, so it makes no request. */
	setWorkingOffline(value: boolean) {
		this.#workingOffline = value;
		this.#send({ type: 'offline', value });
	}

	/** Leaves the election, and stops the worker if this tab owns the catalog. The device's files stay. */
	disconnect() {
		this.#wants = false;
		this.#unsubscribe?.();
		this.#unsubscribe = null;
		this.#yield();
	}

	/**
	 * Answers the API operation `name` for `library` from the device's files, in whichever tab
	 * owns them. Resolves with the JSON the server would send; rejects if the library is not on
	 * the device, nobody answers or the browser has no Web Locks, and, unless this device is `offline`, at once while the
	 * library's copy is being updated, so the caller asks the server.
	 */
	query(library: string, name: string, params: object = {}, offline = false) {
		if (!available()) return Promise.reject(new Error('the catalog is not available'));
		const user = this.#user || undefined;
		return this.#route().ask({ type: 'query', library, name, params, offline, user });
	}

	/**
	 * Calls `listener` with each library whose rows on the device change, announced by whichever
	 * tab owns the catalog, until the returned function is called.
	 */
	onChanged(listener: (library: string) => void) {
		return this.#route().listen(listener);
	}

	/**
	 * Whether `library` is fully on the device: false if it is not or there is no owner to ask.
	 * Online, answers at once when no tab is serving the catalog yet, so the server answers the
	 * read; an owner in another tab that is starting has `STATE_MS` to answer, and one that did
	 * not answer is not asked again for a while. Offline, a tab about to elect waits for its own
	 * owner, since the server cannot answer; so does a caller that found the server unreachable
	 * and says so with `wait`.
	 */
	async onDevice(library: string, wait = false) {
		const offline =
			wait || this.#workingOffline || connectivity.stale || navigator.onLine === false;
		if (!offline && Date.now() < this.#deafUntil) return false;
		// A tab about to elect gives the election a moment rather than answering none: a page
		// that loads first thing, as a launch offline does, would otherwise miss the device.
		let waited = false;
		const end = Date.now() + OWNER_MS;
		while (!(await this.#owned())) {
			// Only a tab still in the election is worth waiting for: one that gave up is not coming.
			if (!offline || !this.#abort || Date.now() >= end) return false;
			waited = true;
			await new Promise((resolve) => setTimeout(resolve, LOOK_MS));
		}
		if (!offline && this.#holding && !this.#serving) return false;
		let unknown = false;
		const answer = await this.#route()
			.ask({ type: 'onDevice', library }, waited ? OWNER_MS : STATE_MS)
			.catch(() => {
				// No answer says nothing about the library: online the server answers the read, and
				// offline the read asks the device and waits, rather than calling it not downloaded.
				unknown = true;
				if (!offline) this.#deafUntil = Date.now() + DEAF_MS;
				return '';
			});
		if (unknown && offline) return true;
		return answer === '1';
	}

	/**
	 * Whether `library`'s search index is built on the device, so a search can be handed to it
	 * without waiting for a build: false where it is not, or where no owner is serving or answers
	 * in `STATE_MS`.
	 */
	async indexed(library: string) {
		if (!available() || !(await this.#owned()) || (this.#holding && !this.#serving)) return false;
		const answer = await this.#route()
			.ask({ type: 'indexed', library }, STATE_MS)
			.catch(() => '');
		return answer === '1';
	}

	/**
	 * Has whichever tab owns the catalog build `library`'s search index in the background, unless
	 * it has one, for a device that has gone offline: its first search then does not wait for it.
	 * Without an owner to answer in time, the first search builds it.
	 */
	prepareIndex(library: string) {
		if (!available()) return;
		this.#route()
			.ask({ type: 'prepareIndex', library })
			.catch(() => {});
	}

	/** Whether some tab holds the lock, which says an owner is at least starting. */
	async #owned() {
		return !!((await navigator.locks?.query()) ?? {}).held?.some(
			(lock) => lock.name === this.#name
		);
	}

	/**
	 * Deletes every file of the catalog from this device, from every tab: the next person to
	 * sign in must see nothing of the last, nor the account and library list kept for starting
	 * offline, nor the images and queue lists kept for it. A tab that owns the catalog does it; if none does, this
	 * tab starts a worker for it.
	 */
	signOut() {
		// A tab owning the catalog lets go of it first, or the wipe would wait on its own lock.
		this.disconnect();
		forget();
		return (this.#signingOut = Promise.all([
			clearImageCache(),
			clearQueueLists(),
			this.#signOut()
		]));
	}

	/** Does the wipe of [`signOut`](#signOut): runs it here if no tab owns the catalog, else asks the owner, trying a few times. */
	async #signOut() {
		if (!available()) return;
		await this.#held;
		for (let attempt = 0; attempt < SIGN_OUT_TRIES; attempt++) {
			const done = await navigator.locks.request(
				this.#name,
				{ ifAvailable: true },
				async (lock) => {
					if (!lock)
						return this.#route()
							.ask({ type: 'signOut' }, SIGN_OUT_MS)
							.then(
								() => true,
								() => false
							);
					this.#launch('');
					await Promise.race([
						this.#call({ type: 'signOut' }).catch((error) => console.error(error)),
						new Promise((resolve) => setTimeout(resolve, TIMEOUT_MS))
					]);
					this.#stop();
					return true;
				}
			);
			if (done) return;
		}
		console.error('another tab did not answer the sign-out; the device keeps its files');
	}

	/** Takes part in the election again, in a tab that gave the catalog up and is back. */
	#resume() {
		if (this.#unsubscribe && !this.#abort) this.#join();
	}

	/**
	 * A tab that is shown joins the election again; one that goes out of sight leaves the queue
	 * for the lock, so only tabs in view are ever waiting, and an owner that goes out of sight
	 * says so, for a waiting tab in view to ask; see [`#turn`](#turn).
	 */
	#visibility() {
		if (!this.#unsubscribe) return;
		if (!this.#hidden()) this.#resume();
		else if (this.#serving) this.#route().say('hidden');
		else if (!this.#holding) this.#yield();
	}

	/**
	 * Hears another tab: a hidden owner heard `want` gives the catalog up, unless its worker is
	 * downloading a snapshot, and wants it back when shown; a waiting tab in view answers `hidden` with `want`.
	 */
	async #turn(kind: Turn) {
		if (kind === 'hidden') {
			if (!this.#hidden() && this.#abort && !this.#serving) this.#route().say('want');
		} else if (this.#hidden() && this.#serving) {
			const busy = await this.#call({ type: 'busy' });
			if (this.#hidden() && this.#serving && !busy) this.#yield();
		}
	}

	/** The channel to the owner, joined when first needed. */
	#route() {
		if (this.#router) return this.#router;
		const router = (this.#router = new CatalogRouter(
			this.#name,
			undefined,
			(request) => request.type === 'signOut' && !this.#serving && this.disconnect()
		));
		router.onTurn((kind) => void this.#turn(kind));
		return router;
	}

	/** Asks the browser, once, not to evict the device's files: only a page can. */
	#persist() {
		if (this.#persisted) return;
		this.#persisted = true;
		navigator.storage?.persist?.().catch((error) => console.error(error));
	}

	/**
	 * Waits for the lock; whoever holds it owns the catalog, for as long as it is held. A tab out
	 * of sight takes it only if no one holds it, and otherwise waits to be shown.
	 */
	#elect() {
		const abort = (this.#abort = new AbortController());
		const released = new Promise<void>((resolve) => (this.#release = resolve));
		const hidden = this.#hidden();
		this.#held = navigator.locks
			.request(this.#name, hidden ? { ifAvailable: true } : { signal: abort.signal }, (lock) => {
				if (lock) return this.#own(released);
				if (this.#abort === abort) this.#abort = this.#release = null;
			})
			.catch((error) => {
				if (error.name !== 'AbortError') console.error(error);
			});
		// A tab in view that has to wait asks a hidden owner for the catalog.
		if (!hidden) this.#route().say('want');
	}

	/** Gives up the wait for the lock, or the lock itself once the worker has released the pool. */
	#yield() {
		this.#abort?.abort();
		this.#release?.();
		this.#abort = this.#release = null;
	}

	/**
	 * Runs while this tab holds the lock: opens the catalog in a new worker, answers requests
	 * from every tab until `released`, then pauses the pool so the next owner can take it.
	 */
	async #own(released: Promise<void>) {
		this.#holding = true;
		this.#hurry = false;
		this.#launch(this.#user);
		// A worker that cannot load leaves the lock to the next tab.
		this.#worker!.onerror = () => this.#yield();
		let started = false;
		const heard = this.#hear('started').then(() => (started = true));
		this.#send({ type: 'start' });
		await Promise.race([heard, released, new Promise((resolve) => setTimeout(resolve, START_MS))]);
		if (!started) this.#yield();
		const stop = started
			? this.#route().serve(async (request: Request) => {
					// Device data never crosses accounts; the asker falls back to the server.
					if (
						request.type === 'query' &&
						request.user !== undefined &&
						request.user !== this.#user
					) {
						void this.#leaveIfStale();
						throw new Error('the catalog is for another account');
					}
					const answer = await this.#call(request);
					// A device wiped for everyone is no one's to keep open.
					if (request.type === 'signOut') this.disconnect();
					return answer;
				})
			: () => {};
		this.#serving = started;
		if (started) this.#deafUntil = 0;
		await released;
		this.#serving = false;
		stop();
		if (started && !this.#hurry) {
			const paused = this.#hear('paused');
			this.#send({ type: 'pause' });
			await Promise.race([paused, new Promise((resolve) => setTimeout(resolve, PAUSE_MS))]);
		}
		this.#stop();
		this.#holding = false;
	}

	/** Gives the catalog up if the session now belongs to someone other than this tab's account; while working offline (no request is made) or on any other failure, keeps it. */
	async #leaveIfStale() {
		if (this.#workingOffline) return;
		try {
			const response = await fetch(`${API}/me`);
			if (response.status === 401) this.disconnect();
			else if (response.ok && String((await response.json()).id) !== this.#user) this.disconnect();
		} catch (error) {
			console.error(error);
		}
	}

	/** Spawns the worker and sends it `init` for `user`; the first message it gets. */
	#launch(user: string) {
		const worker = (this.#worker = this.#spawn());
		worker.onmessage = ({ data }: MessageEvent<FromWorker>) => this.#heard(data);
		this.#send({ type: 'init', api: API, wasm: wasmUrl, user });
		if (this.#workingOffline) this.#send({ type: 'offline', value: true });
	}

	/** Terminates the worker. Requests it had not answered are left for the next owner. */
	#stop() {
		this.#worker?.terminate();
		this.#worker = null;
		this.#calls.clear();
		this.#says.clear();
	}

	/** Posts `message` to the worker, if there is one. */
	#send(message: ToWorker) {
		this.#worker?.postMessage(message);
	}

	/** Sends the worker `request` and resolves with its answer. */
	#call(request: Ask) {
		return new Promise<string>((resolve, reject) => {
			const id = ++this.#callId;
			this.#calls.set(id, { resolve, reject });
			this.#send({ ...request, id });
		});
	}

	/** Resolves when the worker says `type`. */
	#hear(type: 'started' | 'paused') {
		return new Promise<void>((resolve) => this.#says.set(type, resolve));
	}

	/** Handles a message from the worker: logs an error, with a toast if the device is full, or answers who waits. */
	#heard(message: FromWorker) {
		if (message.type === 'error') {
			console.error(message.message);
			if (message.quota) toast('There is no room on this device for the music library’s details.');
		} else if (message.type === 'changed') {
			this.#route().announce(message.libraryId);
		} else if (message.type === 'answer') {
			const call = this.#calls.get(message.id);
			this.#calls.delete(message.id);
			if (message.error === undefined) call?.resolve(message.result ?? '');
			else call?.reject(new Error(message.error));
		} else {
			this.#says.get(message.type)?.();
		}
	}
}

/** The page's one catalog client. */
export const catalog = new CatalogClient();
