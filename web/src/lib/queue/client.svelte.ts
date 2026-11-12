// The page's side of the queue worker (`worker.ts`): the queue's state to render, and the edits
// to send it. The queue itself, which can be hundreds of thousands of items, never leaves the
// worker. The worker's starts, edits, restores and kept queues' deletions go through here to the
// page's one log, which sends them, and what became of each goes back to it. The worker also keeps
// the account's kept queues, which the Kept tab lists from here.

import { API } from '$lib/api/client';
import { connectivity } from '$lib/connectivity.svelte';
import { toast } from '$lib/toast.svelte';
import wasmUrl from '$lib/core-wasm/jewelcase_core_wasm_bg.wasm?url';
import type { KeptQueue, RealtimeServerMessage, SentEdit } from '$lib/api/types';
import type { marks as Marks } from '$lib/marks.svelte';
import type { Realtime } from '$lib/realtime.svelte';
import type { Mutation } from '$lib/sync/mutation';
import { type Result, type Settled, sender } from '$lib/sync/sender';
import type {
	FromWorker,
	Parked,
	QueueMutation,
	Refusal,
	RestoreResult,
	Repeat,
	RowSection,
	RowsWindow,
	StartRequest,
	Summary,
	ToWorker
} from './protocol';

/**
 * The realtime messages the queue needs: `hello` says to catch up, and `queuesChanged` to read the
 * kept queues again.
 */
const QUEUE_MESSAGES = new Set(['hello', 'queueEdit', 'queueReplaced', 'queuesChanged']);

/** Starts the queue worker. */
const spawn = () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });

/** The kinds of mutation the worker gives the log, whose outcomes go back to it. */
const WORKER_MUTATIONS = new Set(['startQueue', 'editQueue', 'restoreQueue', 'deleteKeptQueue']);

/** What of the page's log the queue uses: where its mutations go, and what became of them. */
export type Log = {
	enqueue(name: QueueMutation['name'], args: QueueMutation['args']): unknown;
	onSettled(listener: (rec: Mutation, outcome: Settled, result: Result) => void): () => void;
};

export class QueueClient {
	/** The account's queue; null before it is loaded, and when the account has none. */
	queueId = $state<string | null>(null);
	/** What the queue started from, for a play to record; null with no queue. */
	context = $state<Summary['context']>(null);
	/** Counts up with every change. */
	revision = $state(0);
	/** The item playing now. */
	current = $state<string | null>(null);
	/** The item playing came from the Manual Queue, for a play to record. */
	playingManual = $state(false);
	/** The item playing in the server's queue, as far as is heard: what a report may name. */
	held = $state<string | null>(null);
	/** The server's version of the queue, as far as this device has applied it. */
	version = $state(0);
	/** How many rows the Manual Queue, all of Up Next and History hold. */
	counts = $state({ manual: 0, upNext: 0, history: 0 });
	repeat = $state<Repeat>('off');
	shuffled = $state(false);
	/** The latest edit that did not stay, for the UI to explain. */
	refusal = $state<Refusal | null>(null);
	/**
	 * The kept queues as the device holds them, most recently active first; null until the worker
	 * has read them.
	 */
	kept = $state.raw<KeptQueue[] | null>(null);

	#spawn: () => Worker;
	#log: Log;
	#worker: Worker | null = null;
	#unsubscribe: (() => void) | null = null;
	/** What waits for the worker's answer to a start, a restore or rows, by the asking message's ID. */
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- waiting askers are not state to render
	#waiting = new Map<number, (answer: never) => void>();
	#askId = 0;

	/**
	 * Makes a client that starts its worker with `create` on `connect`, and gives its mutations to
	 * `log`, the page's sender; tests pass fakes.
	 */
	constructor(
		create: () => Worker = spawn,
		log: Log = {
			enqueue: (name, args) => sender.enqueue(name, args as never),
			onSettled: (listener) => sender.onSettled(listener)
		}
	) {
		this.#spawn = create;
		this.#log = log;
	}

	/**
	 * Starts the worker for `user`'s queue and feeds it the queue's realtime messages, and each
	 * hide and unhide `marks` shows, whose rows it leaves out, until [`disconnect`](#disconnect).
	 */
	connect(realtime: Pick<Realtime, 'on'>, user: string, marks?: Pick<typeof Marks, 'onHidden'>) {
		if (this.#worker) return;
		const worker = (this.#worker = this.#spawn());
		worker.onmessage = ({ data }: MessageEvent<FromWorker>) => this.#heard(data);
		this.#send({ type: 'init', api: API, wasm: wasmUrl, user });
		if (connectivity.workOffline) this.#send({ type: 'offline', value: true });
		const unsubscribe = realtime.on((message: RealtimeServerMessage) => {
			if (QUEUE_MESSAGES.has(message.type)) this.#send({ type: 'message', message });
		});
		const unhear = marks?.onHidden((libraryId, trackId, hidden) =>
			this.#send({ type: 'hidden', libraryId, trackId, hidden })
		);
		// What became of the worker's mutations, this tab's own or adopted from a closed one.
		const unsettle = this.#log.onSettled(({ name, args }, outcome, result) => {
			if (!WORKER_MUTATIONS.has(name)) return;
			const mutation = { name, args } as QueueMutation;
			this.#send({ type: 'settled', mutation, outcome, result });
		});
		/**
		 * Has the worker keep the queue when the page becomes hidden, so a page going away keeps it
		 * first.
		 */
		const hide = () => document.visibilityState === 'hidden' && void this.save();
		globalThis.document?.addEventListener('visibilitychange', hide);
		this.#unsubscribe = () => {
			unsubscribe();
			unhear?.();
			unsettle();
			globalThis.document?.removeEventListener('visibilitychange', hide);
		};
	}

	/**
	 * Tells the worker whether the device is working offline, so it makes no request until it is not.
	 */
	setWorkingOffline(value: boolean) {
		this.#send({ type: 'offline', value });
	}

	/** Stops the worker, and with it the queue held on this device. */
	disconnect() {
		this.#unsubscribe?.();
		this.#unsubscribe = null;
		this.#worker?.terminate();
		this.#worker = null;
		this.#waiting.clear();
		this.queueId = null;
		this.context = null;
		this.current = null;
		this.playingManual = false;
		this.held = null;
		this.version = 0;
		this.counts = { manual: 0, upNext: 0, history: 0 };
		this.repeat = 'off';
		this.shuffled = false;
		this.kept = null;
	}

	/**
	 * Applies `edit` here at once and sends it to the server in the background. An `add` of a
	 * `recipe` shows as `length` rows, the view's count, until the server says how many it made.
	 * The worker is told whether the app counts itself offline, as it resolves a search on the
	 * device only then.
	 */
	edit(edit: SentEdit, length?: number) {
		this.#send({ type: 'edit', edit, length, offline: connectivity.state !== 'online' });
	}

	/**
	 * Starts a queue here (`QueueStore.start`): settles once the worker has it, and then
	 * `queueId`, `current` and the rest are the new queue's. False if the start is not a position
	 * of its source. Never settles if the worker is stopped first. The worker is told whether the
	 * app counts itself offline, as with `edit`.
	 */
	start(request: StartRequest): Promise<boolean> {
		return this.#ask((id) => ({
			type: 'start',
			id,
			request: { ...request, offline: connectivity.state !== 'online' }
		}));
	}

	/**
	 * Makes a kept queue the account's active one (`QueueStore.restore`), from the state the device
	 * holds, online or not: settles once the worker has it, and then the queue's state is the
	 * restored queue's, or with why it was not. A state not on the device is asked of the server
	 * unless the app counts itself offline. `parked` is where playback was in the queue it replaces.
	 * Never settles if the worker is stopped first.
	 */
	restore(queueId: string, parked?: Parked): Promise<RestoreResult> {
		const offline = connectivity.state !== 'online';
		return this.#ask((id) => ({ type: 'restore', id, queueId, offline, parked }));
	}

	/** Deletes a kept queue here at once; the log sends the deletion (`QueueStore.deleteKept`). */
	deleteKept(queueId: string) {
		this.#send({ type: 'deleteKept', queueId });
	}

	/**
	 * Asks the worker for up to `limit` rows of `section` from `offset`. Rows it is still
	 * learning come back `unknown`; ask again when `revision` changes. Never answered if the
	 * worker is stopped first.
	 */
	rows(section: RowSection, offset: number, limit: number): Promise<RowsWindow> {
		return this.#ask((id) => ({ type: 'rows', id, section, offset, limit }));
	}

	/**
	 * Has the worker write the queue to the database for the next time the app opens: settles once
	 * the write is done or has failed, and every start and edit made before is in the log, at once
	 * with no worker. Never settles if the worker is stopped first, so a caller should not wait long.
	 */
	save(): Promise<void> {
		if (!this.#worker) return Promise.resolve();
		return this.#ask((id) => ({ type: 'save', id }));
	}

	/** Sends the message `make` builds with a fresh ID, settling with the worker's answer to it. */
	#ask<T>(make: (id: number) => ToWorker): Promise<T> {
		const id = ++this.#askId;
		return new Promise<T>((resolve) => {
			this.#waiting.set(id, resolve);
			this.#send(make(id));
		});
	}

	/** Posts a message to the worker, if it is running. */
	#send(message: ToWorker) {
		this.#worker?.postMessage(message);
	}

	/** Takes in what the worker said. */
	#heard(message: FromWorker) {
		if (
			message.type === 'rows' ||
			message.type === 'started' ||
			message.type === 'restored' ||
			message.type === 'saved'
		) {
			const answer =
				message.type === 'rows'
					? message.window
					: message.type === 'started'
						? message.ok
						: message.type === 'restored'
							? message.result
							: undefined;
			this.#waiting.get(message.id)?.(answer as never);
			this.#waiting.delete(message.id);
			return;
		}
		if (message.type === 'notice') {
			toast(message.message);
			return;
		}
		if (message.type === 'enqueue') {
			this.#log.enqueue(message.mutation.name, message.mutation.args);
			return;
		}
		if (message.type === 'refused') {
			this.refusal = message.refusal;
			return;
		}
		if (message.type === 'kept') {
			this.kept = message.entries;
			return;
		}
		const { summary } = message;
		this.queueId = summary.queueId;
		this.context = summary.context;
		this.revision = summary.revision;
		this.current = summary.current;
		this.playingManual = summary.playingManual;
		this.held = summary.held;
		this.version = summary.version;
		this.repeat = summary.repeat;
		this.shuffled = summary.shuffled;
		this.counts = { manual: summary.manual, upNext: summary.upNext, history: summary.history };
	}
}

export const queue = new QueueClient();
