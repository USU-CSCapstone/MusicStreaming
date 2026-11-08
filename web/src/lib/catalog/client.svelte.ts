// The page's side of the catalog worker (`worker.ts`): it tells the worker who is signed in and
// what the realtime connection hears, and shows what the worker cannot fix itself.

import { API } from '$lib/api/client';
import wasmUrl from '$lib/catalog-wasm/jewelcase_catalog_wasm_bg.wasm?url';
import type { RealtimeServerMessage } from '$lib/api/types';
import type { Realtime } from '$lib/realtime.svelte';
import { toast } from '$lib/toast.svelte';
import type { FromWorker, ToWorker } from './protocol';

/** The realtime messages that make the worker pull. */
const PULLING = new Set(['hello', 'libraryChanged', 'accountChanged']);

/** Starts the catalog worker. */
const spawn = () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });

/** The page's handle on the catalog worker. */
export class CatalogClient {
	#spawn: () => Worker;
	#worker: Worker | null = null;
	#unsubscribe: (() => void) | null = null;
	#persisted = false;

	/** Makes a client that starts its worker with `create`; tests pass a fake. */
	constructor(create: () => Worker = spawn) {
		this.#spawn = create;
	}

	/**
	 * Starts the worker for `user` and feeds it the realtime messages that make it pull, until
	 * [`disconnect`](#disconnect).
	 */
	connect(realtime: Pick<Realtime, 'on'>, user: string) {
		// Also stops a worker that was started only to delete the files, which this account's
		// worker deletes the rest of anyway.
		this.disconnect();
		this.#persist();
		this.#start(user);
		this.#send({ type: 'start' });
		this.#unsubscribe = realtime.on((message: RealtimeServerMessage) => {
			if (PULLING.has(message.type)) this.#send({ type: 'message', message });
		});
	}

	/** Stops the worker. The device's files stay. */
	disconnect() {
		this.#unsubscribe?.();
		this.#unsubscribe = null;
		this.#worker?.terminate();
		this.#worker = null;
	}

	/** Deletes every file of the catalog from this device, for a browser no one is signed in on. */
	signOut() {
		if (!this.#worker) this.#start('');
		this.#send({ type: 'signOut' });
	}

	/** Asks the browser, once, not to evict the device's files: only a page can. */
	#persist() {
		if (this.#persisted) return;
		this.#persisted = true;
		navigator.storage?.persist?.().catch((error) => console.error(error));
	}

	/** Spawns the worker and sends it `init` for `user`; the first message it gets. */
	#start(user: string) {
		const worker = (this.#worker = this.#spawn());
		worker.onmessage = ({ data }: MessageEvent<FromWorker>) => this.#heard(data);
		this.#send({ type: 'init', api: API, wasm: wasmUrl, user });
	}

	/** Posts `message` to the worker, if there is one. */
	#send(message: ToWorker) {
		this.#worker?.postMessage(message);
	}

	/**
	 * Handles a message from the worker: logs an error, with a toast if the device is full, and
	 * stops a worker that was started only to delete the files once it says it has.
	 */
	#heard(message: FromWorker) {
		if (message.type === 'error') {
			console.error(message.message);
			if (message.quota) toast('There is no room on this device for the music library’s details.');
		} else if (!this.#unsubscribe) {
			// A worker started only to delete the files has done it.
			this.disconnect();
		}
	}
}

export const catalog = new CatalogClient();
