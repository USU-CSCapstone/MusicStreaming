// The page's side of the catalog: one tab at a time owns the catalog and runs its worker
// (`worker.ts`); every tab's `query` and `signOut` reach the owner, through `router.ts` when the
// tab is not it. The owner tells the worker who is signed in and what the realtime connection
// hears, and shows what the worker cannot fix itself.

import { API, ApiError, getMe } from '$lib/api/client';
import wasmUrl from '$lib/catalog-wasm/jewelcase_catalog_wasm_bg.wasm?url';
import type { RealtimeServerMessage } from '$lib/api/types';
import type { Realtime } from '$lib/realtime.svelte';
import { toast } from '$lib/toast.svelte';
import type { Ask, FromWorker, Request, ToWorker } from './protocol';
import { CHANNEL, CatalogRouter, TIMEOUT_MS } from './router';

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

/** Whether the browser can elect an owner at all; without Web Locks (plain HTTP, old iOS) the catalog is unavailable. */
const available = () => !!navigator.locks;

/** Starts the catalog worker. */
const spawn = () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });

/** The page's handle on the catalog, whichever tab owns it. */
export class CatalogClient {
	#spawn: () => Worker;
	#name: string;
	#router: CatalogRouter | null = null;
	#worker: Worker | null = null;
	#user = '';
	#unsubscribe: (() => void) | null = null;
	#persisted = false;
	#listening = false;
	/** The sign-out under way, which this tab's next election waits for. */
	#signingOut: Promise<unknown> = Promise.resolve();
	/** This tab answers requests as the owner. */
	#serving = false;
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

	/** Makes a client whose owner starts its worker with `create`, on the lock and channel `name`; tests pass their own. */
	constructor(create: () => Worker = spawn, name = CHANNEL) {
		this.#spawn = create;
		this.#name = name;
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
			addEventListener('pageshow', (event) => event.persisted && this.#resume());
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

	/** Leaves the election, and stops the worker if this tab owns the catalog. The device's files stay. */
	disconnect() {
		this.#unsubscribe?.();
		this.#unsubscribe = null;
		this.#yield();
	}

	/**
	 * Answers the API operation `name` for `library` from the device's files, in whichever tab
	 * owns them. Resolves with the JSON the server would send; rejects if the library is not on
	 * the device, nobody answers, or the browser has no Web Locks.
	 */
	query(library: string, name: string, params: object = {}) {
		if (!available()) return Promise.reject(new Error('the catalog is not available'));
		const user = this.#user || undefined;
		return this.#route().ask({ type: 'query', library, name, params, user });
	}

	/**
	 * Deletes every file of the catalog from this device, from every tab: the next person to
	 * sign in must see nothing of the last. A tab that owns the catalog does it; if none does,
	 * this tab starts a worker for it.
	 */
	signOut() {
		return (this.#signingOut = this.#signOut());
	}

	/** Does [`signOut`](#signOut). */
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
		return (this.#router ??= new CatalogRouter(
			this.#name,
			undefined,
			(request) => request.type === 'signOut' && !this.#serving && this.disconnect()
		));
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
		await released;
		this.#serving = false;
		stop();
		if (started) {
			const paused = this.#hear('paused');
			this.#send({ type: 'pause' });
			await Promise.race([paused, new Promise((resolve) => setTimeout(resolve, PAUSE_MS))]);
		}
		this.#stop();
	}

	/** Gives the catalog up if the session now belongs to someone other than this tab's account; offline or on any other failure, keeps it. */
	async #leaveIfStale() {
		try {
			if ((await getMe(fetch)).id !== this.#user) this.disconnect();
		} catch (error) {
			if (error instanceof ApiError && error.status === 401) this.disconnect();
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

export const catalog = new CatalogClient();
