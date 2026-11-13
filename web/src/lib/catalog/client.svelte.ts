// The page's side of the catalog: one tab at a time owns the catalog and runs its worker
// (`worker.ts`); every tab's `query` and `signOut` reach the owner, through `router.ts` when the
// tab is not it. The owner tells the worker who is signed in and what the realtime connection
// hears, and shows what the worker cannot fix itself. The worker says what each library stands
// at, and the owner tells every tab, which holds it: `onDevice` asks no one.

import { API } from '$lib/api/root';
import { clearImageCache, tellServiceWorker } from '$lib/image-cache';
import { clearQueueLists } from '$lib/queue/lists';
import { forget } from '$lib/last-known';
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
import { CHANNEL, CatalogRouter, TIMEOUT_MS, Unanswered, realClock, type Clock } from './router';

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
	#clock: Clock;
	#router: CatalogRouter | null = null;
	/** The changes announced since this tab began listening, for pages to ask what a read missed. */
	#changes = new Changes();
	#worker: Worker | null = null;
	#user = '';
	#unsubscribe: (() => void) | null = null;
	#persisted = false;
	#listening = false;
	/** The sign-out under way, which this tab's next election waits for. */
	#signingOut: Promise<unknown> = Promise.resolve();
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

	/**
	 * Makes a client whose owner starts its worker with `create`, on the lock and channel `name`;
	 * tests pass their own, and a clock they run by hand.
	 */
	constructor(create: () => Worker = spawn, name = CHANNEL, clock: Clock = realClock) {
		this.#spawn = create;
		this.#name = name;
		this.#clock = clock;
	}

	/**
	 * Takes part in electing the tab that owns the catalog for `user`, and feeds the realtime
	 * messages that make it pull to the worker if this tab wins, until
	 * [`disconnect`](#disconnect).
	 */
	connect(realtime: Pick<Realtime, 'on'>, user: string) {
		this.disconnect();
		this.#persist();
		this.#user = user;
		if (!this.#listening) {
			this.#listening = true;
			// A page that is going away or frozen gives the catalog up, so a tab in front can take it.
			addEventListener('pagehide', () => this.#yield());
			addEventListener('pageshow', (event) => event.persisted && this.#restored());
			// iOS fires no `pagehide` for a tab switch, so an owner out of sight hands over when
			// another tab is waiting; desktop tab switches with no one waiting change nothing.
			document.addEventListener('visibilitychange', () => this.#visibility());
		}
		this.#unsubscribe = realtime.on((message: RealtimeServerMessage) => {
			if (PULLING.has(message.type)) this.#send({ type: 'message', message });
		});
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
	 * Leaves the election, and stops the worker if this tab owns the catalog. The device's files
	 * stay.
	 */
	disconnect() {
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
	 * Whether an owner of another version of the app is there that this tab cannot talk to, so the
	 * app needs updating to read the device.
	 */
	get outdated() {
		return this.#route().foreign && !this.#state?.serving;
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
	 * images until it is told who is signed in. A tab that owns the catalog does it; if none does,
	 * this tab starts a worker for it.
	 */
	signOut() {
		// A tab owning the catalog lets go of it first, or the wipe would wait on its own lock.
		this.disconnect();
		forget();
		return (this.#signingOut = Promise.all([
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
	 * A page restored from the back/forward cache may have missed what the owner published
	 * meanwhile: it asks the owner for its state again, then takes part again.
	 */
	#restored() {
		this.#router?.who();
		this.#resume();
	}

	/**
	 * Gives the catalog up when hidden with another tab waiting for it, unless its worker is
	 * downloading a snapshot, and wants it back when shown.
	 */
	async #visibility() {
		if (!this.#unsubscribe) return;
		if (!document.hidden) return this.#resume();
		if (!this.#serving) return;
		const { pending } = await navigator.locks.query();
		if (!pending?.some((lock) => lock.name === this.#name)) return;
		const busy = await this.#call({ type: 'busy' });
		if (document.hidden && this.#serving && !busy) this.#yield();
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
		router.listen((library, kinds) => this.#changes.record(library, kinds));
		router.onState((state, own) => {
			// No other tab serves while this one does: a state heard from one was sent before this
			// tab took over, and reached it late, as another channel's message can.
			if (this.#serving && !own) return;
			this.#state = state;
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

	/** Waits for the lock; whoever holds it owns the catalog, for as long as it is held. */
	#elect() {
		const abort = (this.#abort = new AbortController());
		const released = new Promise<void>((resolve) => (this.#release = resolve));
		this.#held = navigator.locks
			.request(this.#name, { signal: abort.signal }, () => this.#own(released))
			.catch((error) => {
				if (error.name !== 'AbortError') console.error(error);
			});
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
	 * from every tab until `released`, then pauses the pool so the next owner can take it.
	 */
	async #own(released: Promise<void>) {
		this.#launch(this.#user);
		// A worker that cannot load leaves the lock to the next tab.
		this.#worker!.onerror = () => this.#yield();
		let started = false;
		const heard = this.#hear('started').then(() => (started = true));
		this.#send({ type: 'start' });
		await Promise.race([heard, released, this.#after(START_MS)]);
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
		if (started) this.#serve(true);
		await released;
		this.#serve(false);
		stop();
		if (started) {
			const paused = this.#hear('paused');
			this.#send({ type: 'pause' });
			await Promise.race([paused, this.#after(PAUSE_MS)]);
		}
		this.#stop();
	}

	/** Starts or stops this tab's serving of the catalog, and tells every tab. */
	#serve(on: boolean) {
		if (this.#serving === on) return;
		this.#serving = on;
		this.#publish();
	}

	/** Tells every tab, this one included, what this tab's worker holds, as the owner. */
	#publish() {
		this.#route().publish({
			protocol: PROTOCOL,
			user: this.#user,
			serving: this.#serving,
			heavy: this.#heavy,
			libraries: this.#libraries
		});
	}

	/**
	 * Gives the catalog up if the session now belongs to someone other than this tab's account;
	 * offline or on any other failure, keeps it.
	 */
	async #leaveIfStale() {
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
			this.#says.get(message.type)?.();
		}
	}
}

/** The page's one catalog client. */
export const catalog = new CatalogClient();
