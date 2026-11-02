// The page's side of the queue worker (`worker.ts`): the queue's state to render, and the edits
// to send it. The queue itself, which can be hundreds of thousands of items, never leaves the
// worker.

import { API } from '$lib/api/client';
import type { RealtimeServerMessage, SentEdit } from '$lib/api/types';
import type { Realtime } from '$lib/realtime.svelte';
import type { FromWorker, Refusal, Repeat, Summary, ToWorker } from './protocol';

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
	/** How many rows the Manual Queue, all of Up Next and History hold. */
	counts = $state({ manual: 0, upNext: 0, history: 0 });
	repeat = $state<Repeat>('off');
	shuffled = $state(false);
	/** The latest edit that did not stay, for the UI to explain. */
	refusal = $state<Refusal | null>(null);

	#spawn: () => Worker;
	#worker: Worker | null = null;
	#unsubscribe: (() => void) | null = null;

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
		this.queueId = null;
		this.context = null;
		this.current = null;
		this.counts = { manual: 0, upNext: 0, history: 0 };
		this.repeat = 'off';
		this.shuffled = false;
	}

	/** Applies `edit` here at once and sends it to the server in the background. */
	edit(edit: SentEdit) {
		this.#send({ type: 'edit', edit });
	}

	/** Posts a message to the worker, if it is running. */
	#send(message: ToWorker) {
		this.#worker?.postMessage(message);
	}

	/** Takes in what the worker said. */
	#heard(message: FromWorker) {
		if (message.type === 'refused') {
			this.refusal = message.refusal;
			return;
		}
		const { summary } = message;
		this.queueId = summary.queueId;
		this.context = summary.context;
		this.revision = summary.revision;
		this.current = summary.current;
		this.repeat = summary.repeat;
		this.shuffled = summary.shuffled;
		this.counts = { manual: summary.manual, upNext: summary.upNext, history: summary.history };
	}
}

export const queue = new QueueClient();
