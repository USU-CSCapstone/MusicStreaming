// The page's side of the catalog: one tab at a time owns the catalog and runs its worker
// (`worker.ts`); every tab's `query` and `signOut` reach the owner, through `router.ts` when the
// tab is not it. The owner tells the worker who is signed in and what the realtime connection
// hears, and shows what the worker cannot fix itself. The worker says what each library stands
// at, and the owner tells every tab, which holds it: `onDevice` and `indexed` ask no one.

import { API } from '$lib/api/root';
import { connectivity } from '$lib/connectivity.svelte';
import { clearImageCache, tellServiceWorker } from '$lib/image-cache';
import { clearQueueLists } from '$lib/queue/lists';
import { deviceLibraries, forget, owner, rememberDevice } from '$lib/last-known';
import type { Local } from '$lib/sync/mutation';
import { DISCARDED, IdbPort, unsent, wipeDatabases } from '$lib/sync/store';
import wasmUrl from '$lib/catalog-wasm/jewelcase_catalog_wasm_bg.wasm?url';
import type { RealtimeServerMessage } from '$lib/api/types';
import type { Realtime } from '$lib/realtime.svelte';
import { toast } from '$lib/toast.svelte';
import type { Kind } from './kinds';
import {
	PROTOCOL,
	type Ask,
	type CatalogState,
	type FromWorker,
	type Request,
	type ToWorker
} from './protocol';
import { Changes } from './changes';
import {
	CHANNEL,
	CatalogRouter,
	TIMEOUT_MS,
	Unanswered,
	realClock,
	type Clock,
	type Turn
} from './router';

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
/** How long an online read waits for the device before the server answers it instead. */
const DEVICE_MS = 1500;
/**
 * How long a read made while the server cannot be asked waits for the catalog's owner to be
 * serving, when the device is known to hold the library: an app launched at a deep link has
 * not had its own election or worker finish.
 */
const COLD_MS = 5000;

/**
 * Whether the browser can elect an owner at all; without Web Locks (plain HTTP, old iOS) the
 * catalog is unavailable.
 */
const available = () => !!navigator.locks;

/** Starts the catalog worker. */
const spawn = () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });

/** The page's handle on the catalog, whichever tab owns it. */
export class CatalogClient {
	#spawn: () => Worker;
	#name: string;
	/** Whether this tab is out of sight; tests give each tab its own. */
	#hidden: () => boolean;
	#clock: Clock;
	#router: CatalogRouter | null = null;
	/** The changes announced since this tab began listening, for pages to ask what a read missed. */
	#changes = new Changes();
	#worker: Worker | null = null;
	#user = '';
	#unsubscribe: (() => void) | null = null;
	#persisted = false;
	#listening = false;
	/** This tab is in the election, or about to be: it will own the catalog or another tab will. */
	#wants = false;
	/** The sign-out under way, which this tab's next election waits for. */
	#signingOut: Promise<unknown> = Promise.resolve();
	/** This tab holds the lock, whether or not its worker has started. */
	#holding = false;
	/** The page is about to freeze: the owner stops its worker without waiting for it. */
	#hurry = false;
	/** This tab answers requests as the owner. */
	#serving = false;
	/** What the owner last told every tab of the catalog; null until one has. */
	#state = $state.raw<CatalogState | null>(null);
	/**
	 * What this tab's worker last said of each library, which this tab tells the others while it owns
	 * the catalog.
	 */
	#libraries: CatalogState['libraries'] = {};
	/**
	 * Whether this tab's worker is inside a long call, which this tab tells the others while it owns
	 * the catalog.
	 */
	#heavy = false;
	/** The reads waiting for the owner to serve, each given whether it does. */
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- waiting readers are not state to render
	#waiting = new Set<(serving: boolean) => void>();
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
	/** The libraries the worker found a file for at its start, if it said. */
	#found: string[] | undefined;

	/**
	 * Makes a client whose owner starts its worker with `create`, on the lock and channel `name`;
	 * tests pass their own, `hidden` and a clock they run by hand.
	 */
	constructor(
		create: () => Worker = spawn,
		name = CHANNEL,
		hidden = () => document.hidden,
		clock: Clock = realClock
	) {
		this.#spawn = create;
		this.#name = name;
		this.#hidden = hidden;
		this.#clock = clock;
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
			addEventListener('pageshow', (event) => event.persisted && this.#restored());
			// Chromium freezes a hidden page without `pagehide`, and a frozen owner must not keep the
			// lock: its worker is stopped without the pause round trip, which needs a task the
			// frozen page never runs, so the lock goes with the microtasks that follow.
			document.addEventListener('freeze', () => {
				this.#hurry = true;
				this.#yield();
			});
			document.addEventListener('resume', () => this.#restored());
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

	/**
	 * Tells the worker, if this tab owns one, whether the device is working offline, so it makes no
	 * request.
	 */
	setWorkingOffline(value: boolean) {
		this.#send({ type: 'offline', value });
	}

	/**
	 * Leaves the election, and stops the worker if this tab owns the catalog. The device's files
	 * stay.
	 */
	disconnect() {
		this.#wants = false;
		this.#unsubscribe?.();
		this.#unsubscribe = null;
		this.#yield();
	}

	/**
	 * Answers the API operation `name` for `library` from the device's files, in whichever tab
	 * owns them. Resolves with the JSON the server would send; rejects if the library is not on
	 * the device, nobody answers or the browser has no Web Locks, and, unless this device is
	 * `offline`, at once while the library's copy is being updated and after `DEVICE_MS` with no
	 * answer, so the caller asks the server. An owner that did not answer is not asked again
	 * until it says it is there.
	 */
	async query(library: string, name: string, params: object = {}, offline = false) {
		if (!available()) throw new Error('the catalog is not available');
		const user = this.#user || undefined;
		try {
			return await this.#route().ask(
				{ type: 'query', library, name, params, offline, user },
				offline ? undefined : DEVICE_MS
			);
		} catch (error) {
			if (error instanceof Unanswered && !offline) this.#doubt();
			throw error;
		}
	}

	/**
	 * How many changes have been announced; take it before reading, and give it to `changedSince`.
	 */
	get changes() {
		this.#route();
		return this.#changes.count;
	}

	/** Whether `library` changed in any of `kinds` after `changes` was `count`, as announced. */
	changedSince(count: number, library: string, kinds: readonly Kind[]) {
		return this.#changes.since(count, library, kinds);
	}

	/**
	 * Calls `listener` with each library whose rows on the device change, and the kinds that
	 * changed, announced by whichever tab owns the catalog, until the returned function is called.
	 */
	onChanged(listener: (library: string, kinds: readonly Kind[]) => void) {
		return this.#route().listen(listener);
	}

	/**
	 * Whether `library` is fully on the device, as the owner last said: false if it is not, if
	 * no owner of this tab's account is serving, or while the owner is inside one long call, so an
	 * online read goes to the server at once. Asks no one.
	 */
	onDevice(library: string) {
		const state = this.#current();
		const here = state && !state.heavy && state.libraries[library];
		return !!here && here.onDevice && !here.applying;
	}

	/**
	 * The owner's state if it is serving for this tab's account, else nothing: any other owner is no
	 * owner to this tab.
	 */
	#current() {
		this.#route();
		const state = this.#state;
		return state?.serving && state.user === this.#user ? state : undefined;
	}

	/**
	 * Whether `library` is on the device as `onDevice` says, for a read the server cannot answer:
	 * it waits, up to `COLD_MS`, for an owner that is not serving yet if this device holds the
	 * library, which an app launched offline at a deep link knows from what it kept.
	 */
	async waitForDevice(library: string) {
		if (!available()) return false;
		if (!this.#current()) {
			if (!deviceLibraries(this.#user).has(library)) return false;
			if (!(await this.#serves(COLD_MS))) return false;
		}
		const here = this.#current()?.libraries[library];
		return !!here && here.onDevice;
	}

	/**
	 * Whether `library`'s search index is built on the device, so a search can be handed to it
	 * without waiting for a build: false where it is not, or where no owner is serving. Asks no one.
	 */
	indexed(library: string) {
		const state = this.#current();
		return !!state && !state.heavy && !!state.libraries[library]?.indexed;
	}

	/**
	 * What the owner last said of each library the device holds or left for want of room, as the
	 * account page shows it; empty while no owner of this tab's account is serving.
	 */
	get libraries(): CatalogState['libraries'] {
		return this.#current()?.libraries ?? {};
	}

	/**
	 * Whether an owner of another version of the app is there that this tab cannot talk to, so the
	 * app needs updating to read the device.
	 */
	get outdated() {
		return this.#route().foreign && !this.#state?.serving;
	}

	/**
	 * Has whichever tab owns the catalog build `library`'s search index in the background, unless
	 * it has one, for a device that has gone offline: its first search then does not wait for it.
	 * Without an owner to answer in time, the first search builds it.
	 */
	prepareIndex(library: string) {
		if (!available()) return;
		this.#route()
			.ask({ type: 'prepareIndex', library, user: this.#user || undefined })
			.catch(() => {});
	}

	/**
	 * Has whichever tab owns the catalog make a mutation's writes to its library that the file does
	 * not hold (`$lib/sync`), for `user`. Settles once they are made, or at once where the library
	 * is not on the device as the owner last said, or no owner for `user` is serving: the owner that
	 * holds it next makes them from the log as it starts or downloads it. With `answered`, the
	 * server has answered the mutation: those the file holds are kept through rebases until the
	 * account feed brings them, and none is made. Never rejects.
	 */
	async mutate(user: string, { library, calls }: Local, answered = false) {
		if (!available() || user !== this.#user || !this.#current()?.libraries[library]?.onDevice)
			return;
		await this.#route()
			.ask({ type: 'mutate', library, calls, answered, user })
			.catch((error) => console.error(error));
	}

	/**
	 * Has whichever tab owns the catalog take back the device's own writes to `library` and make
	 * those the log holds again, for `user`. Settles once done, or at once where there is nothing
	 * to do it on, as `mutate`. Never rejects.
	 */
	async rebase(user: string, library: string) {
		if (!available() || user !== this.#user || !this.#current()?.libraries[library]?.onDevice)
			return;
		await this.#route()
			.ask({ type: 'rebase', library, user })
			.catch((error) => console.error(error));
	}

	/**
	 * Tells whichever tab owns the catalog that the server answered a write to `playlistId` of
	 * `library` with it at `version`, for `user`, so a write made twice over the server's rows of it
	 * is made once. Settles once done, or at once where there is nothing to do it on, as `mutate`.
	 * Never rejects.
	 */
	async confirmed(user: string, library: string, playlistId: string, version: number) {
		if (!available() || user !== this.#user || !this.#current()?.libraries[library]?.onDevice)
			return;
		await this.#route()
			.ask({ type: 'confirmed', library, playlistId, version, user })
			.catch((error) => console.error(error));
	}

	/** Resolves with whether the owner is serving, as soon as it says so, or false after `ms`. */
	#serves(ms: number) {
		if (this.#state?.serving) return Promise.resolve(true);
		return new Promise<boolean>((resolve) => {
			const done = (serving: boolean) => {
				cancel();
				this.#waiting.delete(done);
				resolve(serving);
			};
			const cancel = this.#clock.after(ms, () => done(false));
			this.#waiting.add(done);
		});
	}

	/**
	 * Takes the owner that did not answer as gone until it says otherwise, so the reads that
	 * follow go to the server at once: a live owner answers the question and is believed again.
	 */
	#doubt() {
		if (!this.#state?.serving || this.#serving) return;
		this.#state = { ...this.#state, serving: false };
		this.#route().who();
	}

	/** Waits `ms` on this client's clock. */
	#after(ms: number) {
		return new Promise<void>((resolve) => this.#clock.after(ms, resolve));
	}

	/**
	 * Deletes every file of the catalog from this device, from every tab: the next person to
	 * sign in must see nothing of the last, nor the account and library list kept for starting
	 * offline, nor the images and queue lists kept for it, and the service worker keeps no more
	 * images until it is told who is signed in, nor the log's unsent work: every tab lets go of
	 * its database before it is deleted. A tab that owns the catalog does it; if none does, this
	 * tab starts a worker for it. Where the server ended the session (not `chosen`, as a logout
	 * or a sign-in as someone else is), the unsent plays and changes the wipe deletes are counted
	 * first and told once on the login page.
	 */
	signOut(chosen = false) {
		// A tab owning the catalog lets go of it first, or the wipe would wait on its own lock.
		this.disconnect();
		const user = owner();
		forget();
		const wiped = (async () => {
			if (user && !chosen) await discarded(user);
			await wipeDatabases(user ? [user] : []);
		})();
		return (this.#signingOut = Promise.all([
			wiped,
			clearImageCache(),
			clearQueueLists(),
			tellServiceWorker(null),
			this.#signOut()
		]));
	}

	/**
	 * Does the wipe of [`signOut`](#signOut): runs it here if no tab owns the catalog, else asks the
	 * owner, trying a few times.
	 */
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
						this.#after(TIMEOUT_MS)
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
	 * A page restored from the back/forward cache, or unfrozen, may have missed what the owner
	 * published meanwhile: it asks the owner for its state again, then takes part again.
	 */
	#restored() {
		this.#router?.who();
		this.#resume();
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
	 * downloading a snapshot, and wants it back when shown; a waiting tab in view answers `hidden`
	 * with `want`.
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
			(request) => request.type === 'signOut' && !this.#serving && this.disconnect(),
			this.#clock
		));
		router.onTurn((kind) => void this.#turn(kind));
		router.listen((library, kinds) => this.#changes.record(library, kinds));
		router.onState((state, own) => {
			// No other tab serves while this one does: a state heard from one was sent before this
			// tab took over, and reached it late, as another channel's message can.
			if (this.#serving && !own) return;
			this.#state = state;
			if (state.serving) for (const done of [...this.#waiting]) done(true);
		});
		router.who();
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

	/**
	 * Gives up the wait for the lock, or the lock itself once the worker has released the pool; an
	 * owner says so first.
	 */
	#yield() {
		this.#serve(false);
		this.#abort?.abort();
		this.#release?.();
		this.#abort = this.#release = null;
	}

	/**
	 * Runs while this tab holds the lock: opens the catalog in a new worker, answers requests
	 * from every tab until `released`, then pauses the pool so the next owner can take it. A worker
	 * that does not start in `START_MS` gives the lock up, and the tab joins the election again
	 * behind any tab waiting, so the catalog is never left without an owner.
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
		const late = await Promise.race([
			heard.then(() => false),
			released.then(() => false),
			this.#after(START_MS).then(() => true)
		]);
		// Only a hold still this tab's own is given up: once `released`, the tab may already be in
		// a newer election, which this must not end.
		if (late) this.#yield();
		const stop = started
			? this.#route().serve(async (request: Request) => {
					// Device data never crosses accounts, nor does its work; a query's asker falls
					// back to the server.
					if ('user' in request && request.user !== undefined && request.user !== this.#user) {
						void this.#leaveIfStale();
						throw new Error('the catalog is for another account');
					}
					const answer = await this.#call(request);
					// A device wiped for everyone is no one's to keep open.
					if (request.type === 'signOut') this.disconnect();
					return answer;
				})
			: () => {};
		if (started) {
			this.#reportEvicted();
			this.#serve(true);
		}
		await released;
		this.#serve(false);
		stop();
		if (started && !this.#hurry) {
			const paused = this.#hear('paused');
			this.#send({ type: 'pause' });
			await Promise.race([paused, this.#after(PAUSE_MS)]);
		}
		this.#stop();
		this.#holding = false;
		if (late && this.#wants && !this.#abort) this.#join();
	}

	/** Starts or stops this tab's serving of the catalog, and tells every tab. */
	#serve(on: boolean) {
		if (this.#serving === on) return;
		this.#serving = on;
		this.#publish();
	}

	/**
	 * Tells the user once if a library the device held for this account the last time a tab served
	 * it has no file at the worker's start, as when the browser cleared it. A file the worker found
	 * and deleted itself (an older schema, one that would not open) is not reported, and nothing is
	 * after a failed start. Serving rewrites the libraries held, so a later start does not report
	 * the same ones again.
	 */
	#reportEvicted() {
		const found = this.#found;
		if (!found) return;
		const missing = [...deviceLibraries(this.#user)].filter((id) => !found.includes(id));
		if (missing.length === 0) return;
		toast(
			missing.length === 1
				? 'A music library is no longer on this device and will be downloaded again.'
				: `${missing.length} music libraries are no longer on this device and will be downloaded again.`
		);
	}

	/**
	 * Tells every tab, this one included, what this tab's worker holds, as the owner. While it
	 * serves, the libraries it holds are also kept for the next launch, which may be offline.
	 */
	#publish() {
		this.#route().publish({
			protocol: PROTOCOL,
			user: this.#user,
			serving: this.#serving,
			heavy: this.#heavy,
			libraries: this.#libraries
		});
		if (this.#serving && this.#user)
			rememberDevice(
				this.#user,
				Object.keys(this.#libraries).filter((id) => this.#libraries[id].onDevice)
			);
	}

	/**
	 * Gives the catalog up if the session now belongs to someone other than this tab's account; while
	 * working offline (no request is made) or on any other failure, keeps it.
	 */
	async #leaveIfStale() {
		if (connectivity.workOffline) return;
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
		if (connectivity.workOffline) this.#send({ type: 'offline', value: true });
	}

	/** Terminates the worker. Requests it had not answered are left for the next owner. */
	#stop() {
		this.#worker?.terminate();
		this.#worker = null;
		this.#libraries = {};
		this.#heavy = false;
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

	/**
	 * Handles a message from the worker: logs an error, with a toast if the device is full, passes on
	 * what a library stands at, or answers who waits.
	 */
	#heard(message: FromWorker) {
		if (message.type === 'error') {
			console.error(message.message);
			if (message.quota) toast('There is no room on this device for the music library’s details.');
		} else if (message.type === 'changed') {
			this.#route().announce(message.libraryId, message.kinds);
		} else if (message.type === 'heavy') {
			this.#heavy = message.on;
			if (this.#serving) this.#publish();
		} else if (message.type === 'state') {
			this.#libraries = message.libraries;
			if (this.#serving) this.#publish();
		} else if (message.type === 'answer') {
			const call = this.#calls.get(message.id);
			this.#calls.delete(message.id);
			if (message.error === undefined) call?.resolve(message.result ?? '');
			else call?.reject(new Error(message.error));
		} else {
			if (message.type === 'started') this.#found = message.found;
			this.#says.get(message.type)?.();
		}
	}
}

/** The page's one catalog client. */
export const catalog = new CatalogClient();

/**
 * Notes in this tab's session storage how many unsent plays and changes `user`'s log holds, for
 * the login page to tell.
 */
async function discarded(user: string) {
	if (typeof indexedDB === 'undefined') return;
	const port = new IdbPort(user);
	try {
		const count = await unsent(port);
		if (count > 0) sessionStorage.setItem(DISCARDED, String(count));
	} catch {
		// Nothing could be counted, so nothing is told.
	} finally {
		port.close();
	}
}
