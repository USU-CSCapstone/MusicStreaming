// The page's side of the queue worker (`worker.ts`): the queue's state to render, and the few
// things the page can ask of it. The queue itself, which can be hundreds of thousands of items,
// never leaves the worker; the page reads windows of it.

import { API } from '$lib/api/client';
import type { RealtimeServerMessage, SentEdit } from '$lib/api/types';
import type { Realtime } from '$lib/realtime.svelte';
import { toast } from '$lib/toast.svelte';
import type {
	FromWorker,
	ItemTrack,
	List,
	Question,
	Refusal,
	Repeat,
	StartArgs,
	Summary,
	ToWorker
} from './protocol';

/** The realtime messages that carry queue changes, and the one that says to catch up. */
const QUEUE_MESSAGES = new Set(['hello', 'queueEdit', 'queueReplaced']);

/** What to tell the user when an edit they made did not stay (`requirements/realtime.md` §7). */
const EXPLAINED: Record<string, string> = {
	anchor_gone: 'That change did not apply: the song it was placed next to is no longer in Up Next.',
	superseded: 'That change did not apply: a new queue replaced this one.'
};

/** Starts the queue worker. */
const spawn = () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' });

export class QueueClient {
	/** The account's queue; null before it is loaded, and when the account has none. */
	queueId = $state<string | null>(null);
	/** What the queue started from, for a play to record; null with no queue. */
	context = $state<Summary['context']>(null);
	/** Counts up with every change; a window read at an earlier one may be stale. */
	revision = $state(0);
	/** The item playing now. */
	current = $state<string | null>(null);
	/** How many items the Manual Queue, the context's Up Next and History hold. */
	counts = $state({ manual: 0, upcoming: 0, history: 0 });
	/** How the queue repeats. */
	repeat = $state<Repeat>('off');
	/** The context's Up Next is shuffled. */
	shuffled = $state(false);
	/** A start from a list is under way: the queue is its first track until the server's arrives. */
	settling = $state(false);
	/** The server has not yet taken the latest start: nothing it would name can be reported yet. */
	starting = $state(false);
	/** The latest edit or start that was reverted, for the UI to explain. */
	refusal = $state<Refusal | null>(null);

	#spawn: () => Worker;
	#worker: Worker | null = null;
	#unsubscribe: (() => void) | null = null;
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- who awaits what is not state to render
	#asked = new Map<number, { resolve: (value: never) => void; reject: (error: Error) => void }>();
	#asks = 0;

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
		for (const { reject } of this.#asked.values()) reject(new Error('The queue is closed.'));
		this.#asked.clear();
		this.queueId = null;
		this.context = null;
		this.current = null;
		this.counts = { manual: 0, upcoming: 0, history: 0 };
		this.repeat = 'off';
		this.shuffled = false;
		this.settling = false;
		this.starting = false;
	}

	/** Starts a queue, here at once and on the server in the background. */
	start(args: StartArgs) {
		this.#send({ type: 'start', args });
	}

	/**
	 * Applies `edit` here at once and sends it to the server in the background. An `add` of a
	 * `query` is only sent: it shows when the server's answer arrives.
	 */
	edit(edit: SentEdit) {
		this.#send({ type: 'edit', edit });
	}

	/** Up to `limit` item IDs of `list` from `offset`, as the queue is now. */
	window(list: List, offset: number, limit: number): Promise<string[]> {
		return this.#ask({ what: 'window', list, offset, limit });
	}

	/** The track an item plays, or null if the queue has no such item. */
	track(item: string): Promise<ItemTrack | null> {
		return this.#ask({ what: 'track', item });
	}

	/** Posts a message to the worker, if it is running. */
	#send(message: ToWorker) {
		this.#worker?.postMessage(message);
	}

	/** Asks the worker a question; rejects if it is closed first. */
	#ask<T>(question: Question): Promise<T> {
		return new Promise<T>((resolve, reject) => {
			if (!this.#worker) return reject(new Error('The queue is closed.'));
			const id = ++this.#asks;
			this.#asked.set(id, { resolve, reject });
			this.#send({ type: 'ask', id, question });
		});
	}

	/** Takes in what the worker said. */
	#heard(message: FromWorker) {
		if (message.type === 'state') {
			const { summary } = message;
			this.queueId = summary.queueId;
			this.context = summary.context;
			this.revision = summary.revision;
			this.current = summary.current;
			this.repeat = summary.repeat;
			this.shuffled = summary.shuffled;
			this.settling = summary.settling;
			this.starting = summary.starting;
			this.counts = {
				manual: summary.manual,
				upcoming: summary.upcoming,
				history: summary.history
			};
		} else if (message.type === 'refused') {
			this.refusal = message.refusal;
			const { code, edit, message: reason } = message.refusal;
			const said =
				EXPLAINED[code] ?? (edit?.type === 'add' && `Couldn’t add to the queue. ${reason}`);
			if (said) toast(said);
		} else {
			const asked = this.#asked.get(message.id);
			this.#asked.delete(message.id);
			if (message.type === 'answer') asked?.resolve(message.value as never);
			else asked?.reject(new Error(message.error));
		}
	}
}

export const queue = new QueueClient();
