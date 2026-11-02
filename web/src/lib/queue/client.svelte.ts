// The page's side of the queue worker (`worker.ts`): the queue's state to render, and the edits
// to send it. The queue itself, which can be hundreds of thousands of items, never leaves the
// worker.

import { API } from '$lib/api/client';
import type { RealtimeServerMessage, SentEdit } from '$lib/api/types';
import type { Realtime } from '$lib/realtime.svelte';
import type {
	FromWorker,
	Refusal,
	Repeat,
	RowSection,
	RowsWindow,
	StartRequest,
	Summary,
	ToWorker
} from './protocol';

/** The realtime messages the queue needs: `hello` says to catch up. */
const QUEUE_MESSAGES = new Set(['hello', 'queueEdit', 'queueReplaced']);

/** Starts the queue worker. */
const spawn = () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });

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

	#spawn: () => Worker;
	#worker: Worker | null = null;
	#unsubscribe: (() => void) | null = null;
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- waiting askers are not state to render
	#asks = new Map<number, (window: RowsWindow) => void>();
	#askId = 0;
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- waiting askers are not state to render
	#starts = new Map<number, (ok: boolean) => void>();

	/** Makes a client that starts its worker with `create` on `connect`; tests pass a fake. */
	constructor(create: () => Worker = spawn) {
		this.#spawn = create;
	}

	/** Starts the worker and feeds it the queue's realtime messages, until [`disconnect`](#disconnect). */
	connect(realtime: Pick<Realtime, 'on'>) {
		if (this.#worker) return;
		const worker = (this.#worker = this.#spawn());
		worker.onmessage = ({ data }: MessageEvent<FromWorker>) => this.#heard(data);
		this.#send({ type: 'init', api: API });
		this.#unsubscribe = realtime.on((message: RealtimeServerMessage) => {
			if (QUEUE_MESSAGES.has(message.type)) this.#send({ type: 'message', message });
		});
	}

	/** Stops the worker, and with it the queue held on this device. */
	disconnect() {
		this.#unsubscribe?.();
		this.#unsubscribe = null;
		this.#worker?.terminate();
		this.#worker = null;
		this.#asks.clear();
		this.#starts.clear();
		this.queueId = null;
		this.context = null;
		this.current = null;
		this.playingManual = false;
		this.held = null;
		this.version = 0;
		this.counts = { manual: 0, upNext: 0, history: 0 };
		this.repeat = 'off';
		this.shuffled = false;
	}

	/**
	 * Applies `edit` here at once and sends it to the server in the background. An `add` of a
	 * `recipe` shows as `length` rows, the view's count, until the server says how many it made.
	 */
	edit(edit: SentEdit, length?: number) {
		this.#send({ type: 'edit', edit, length });
	}

	/**
	 * Starts a queue here (`QueueStore.start`): settles once the worker has it, and then
	 * `queueId`, `current` and the rest are the new queue's. False if the start is not a position
	 * of its source. Never settles if the worker is stopped first.
	 */
	start(request: StartRequest): Promise<boolean> {
		const id = ++this.#askId;
		return new Promise((resolve) => {
			this.#starts.set(id, resolve);
			this.#send({ type: 'start', id, request });
		});
	}

	/**
	 * Asks the worker for up to `limit` rows of `section` from `offset`. Rows it is still
	 * learning come back `unknown`; ask again when `revision` changes. Never answered if the
	 * worker is stopped first.
	 */
	rows(section: RowSection, offset: number, limit: number): Promise<RowsWindow> {
		const id = ++this.#askId;
		return new Promise((resolve) => {
			this.#asks.set(id, resolve);
			this.#send({ type: 'rows', id, section, offset, limit });
		});
	}

	/** Posts a message to the worker, if it is running. */
	#send(message: ToWorker) {
		this.#worker?.postMessage(message);
	}

	/** Takes in what the worker said. */
	#heard(message: FromWorker) {
		if (message.type === 'rows') {
			this.#asks.get(message.id)?.(message.window);
			this.#asks.delete(message.id);
			return;
		}
		if (message.type === 'started') {
			this.#starts.get(message.id)?.(message.ok);
			this.#starts.delete(message.id);
			return;
		}
		if (message.type === 'refused') {
			this.refusal = message.refusal;
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
