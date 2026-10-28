// The account's queue on this device: the server's queue as confirmed, plus this device's own
// edits that the server has not yet confirmed, replayed on top (`design/queue.md`,
// `requirements/general.md` §3.4). The order is the shared core's, run in WebAssembly, so this
// holds no rule about it. Plain TypeScript so it runs in a worker and in tests; `worker.ts` is
// its shell.

import { Fetched, Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type { PlayContext, QueueEdit, RealtimeServerMessage, StartQueue } from '$lib/api/types';
import type { ItemTrack, List, Question, Refusal, Repeat, StartArgs, Summary } from './protocol';

type Context = PlayContext;

export type Options = {
	/** The API's base path, such as `/api/v1`. */
	api: string;
	fetch?: typeof fetch;
	/** How long to wait before sending again what could not be sent. */
	retryMs?: number;
	/** Called after every change, with what the page shows. */
	changed: (summary: Summary) => void;
	/** Called when an edit or start could not be kept. */
	refused: (refusal: Refusal) => void;
};

/** An edit this device made that the server has not confirmed. */
type Pending = {
	editId: string;
	queueId: string;
	edit: QueueEdit;
	/** The version the server answered it made, once it has. */
	version?: number;
};

type QueueEditMessage = Extract<RealtimeServerMessage, { type: 'queueEdit' }>;

/** The server's problem body, if it sent one. */
type Problem = { code?: string; detail?: string; title?: string } | null;

export class QueueStore {
	#api: string;
	#fetch: typeof fetch;
	#retryMs: number;
	#changed: (summary: Summary) => void;
	#refused: (refusal: Refusal) => void;

	/** The server's queue: its snapshot with every edit since applied, in version order. */
	#confirmed: Queue | null = null;
	/** What this device shows: `#confirmed` with `#pending` on top. The same object with no pending. */
	#view: Queue | null = null;
	#queueId: string | null = null;
	/** What the queue started from, as the server keeps it; null with no queue. */
	#context: Context | null = null;
	#version = 0;
	/**
	 * Whether `#confirmed` came from the server. A queue this device started did not, so it may
	 * differ from the server's, which carries what the device did not know of; the first fetch
	 * after a start is never conditional.
	 */
	#fromServer = false;
	/** The part of the server's `ETag` that is new each time it starts, as last given. */
	#run: string | null = null;
	#revision = 0;
	#pending: Pending[] = [];
	/** A start the server has not yet answered. It goes before anything else is sent. */
	#launch: StartQueue | null = null;
	#refetch = false;
	/** The queue is the one track of a start from a list, until the server's replaces it. */
	#placeholder = false;
	/** Edits heard while the queue is being fetched; the fetch may predate them. */
	#held: QueueEditMessage[] | null = null;
	#draining = false;

	/** Makes an empty store; it loads the server's queue when the connection says hello. */
	constructor({
		api,
		fetch: fetcher = (...args) => fetch(...args),
		retryMs = 2000,
		changed,
		refused
	}: Options) {
		this.#api = api;
		this.#fetch = fetcher;
		this.#retryMs = retryMs;
		this.#changed = changed;
		this.#refused = refused;
	}

	/**
	 * Starts a queue at once and tells the server in the background. The Manual Queue comes
	 * along, as the server carries it. Anything else queued up for the old queue is dropped.
	 * A start from a list's `query` plays just its first track until the server has run the
	 * query: the queue is then fetched, and replaces this one.
	 */
	start(args: StartArgs) {
		const { context, libraryId, shuffleSeed } = args;
		const queueId = crypto.randomUUID();
		const key = crypto.randomUUID();
		const query = 'query' in args;
		const shown = query ? [args.startTrackId] : args.tracks;
		const start = query ? 0 : args.start;
		let next: Queue;
		try {
			// `launch` uses up the queue it replaces, so it gets a copy of the one shown.
			next = Queue.launch(
				JSON.stringify({ key, libraryId, tracks: shown }),
				start,
				shuffleSeed,
				this.#view?.copy()
			);
		} catch (error) {
			this.#refused({ code: 'invalid', message: String(error) });
			return;
		}
		// Edits the server may not have are lost with the old queue, though the carried Manual
		// Queue holds their items: fetch what the server made of it, and say what went.
		const unsent = this.#pending.filter((pending) => pending.version === undefined);
		for (const { edit } of unsent) {
			this.#refused({ code: 'superseded', message: 'A new queue replaced this change.', edit });
		}
		if (unsent.length > 0) this.#refetch = true;
		this.#release();
		this.#confirmed = this.#view = next;
		this.#queueId = queueId;
		this.#context = context;
		this.#version = 0;
		this.#fromServer = false;
		this.#pending = [];
		this.#placeholder = query;
		this.#launch = {
			queueId,
			context,
			...(query
				? { source: { key, libraryId, query: args.query }, startTrackId: args.startTrackId }
				: { source: { key, libraryId, tracks: args.tracks }, start }),
			...(shuffleSeed === undefined ? {} : { shuffle: { seed: shuffleSeed } })
		};
		this.#emit();
		this.#drain();
	}

	/**
	 * Applies `edit` here at once and sends it in the background. An edit the queue refuses
	 * changes nothing and is reported.
	 */
	edit(edit: QueueEdit) {
		if (!this.#view || !this.#queueId) {
			this.#refused({ code: 'no_queue', message: 'There is no queue to change.', edit });
			return;
		}
		const next = this.#view === this.#confirmed ? this.#view.copy() : this.#view;
		try {
			next.apply(JSON.stringify(edit));
		} catch (error) {
			if (next !== this.#view) next.free();
			this.#refused({ code: 'invalid', message: String(error), edit });
			return;
		}
		this.#view = next;
		this.#pending.push({ editId: crypto.randomUUID(), queueId: this.#queueId, edit });
		this.#emit();
		this.#drain();
	}

	/** Takes in what the server said over the realtime connection. */
	message(message: RealtimeServerMessage) {
		if (message.type === 'hello') {
			this.#refetch = true;
			this.#drain();
		} else if (message.type === 'queueReplaced') {
			if (message.queueId !== this.#queueId) {
				this.#refetch = true;
				this.#drain();
			}
		} else if (message.type === 'queueEdit') {
			this.#heard(message);
		}
	}

	/** The answer to a question about the queue as shown now. */
	ask(question: Question): string[] | ItemTrack | null {
		if (question.what === 'track') return this.#track(question.item);
		return this.#window(question.list, question.offset, question.limit);
	}

	/** Up to `limit` item IDs of `list` from `offset`; empty with no queue. */
	#window(list: List, offset: number, limit: number): string[] {
		const view = this.#view;
		if (!view) return [];
		if (list === 'manual') return view.manualWindow(offset, limit);
		return list === 'upcoming' ? view.upcoming(offset, limit) : view.history(offset, limit);
	}

	/** The track an item plays, or null if the queue has no such item. */
	#track(item: string): ItemTrack | null {
		const found = this.#view?.track(item);
		return found ? { libraryId: found[0], trackId: found[1] } : null;
	}

	/** An edit another device (or this one) made, as the server applied it. */
	#heard(message: QueueEditMessage) {
		if (this.#held) {
			this.#held.push(message);
			return;
		}
		if (message.queueId !== this.#queueId || !this.#confirmed) {
			this.#refetch = true;
			this.#drain();
		} else if (message.version > this.#version) {
			if (message.version === this.#version + 1) this.#confirm(this.#confirmed, message);
			else {
				this.#refetch = true;
				this.#drain();
			}
		}
	}

	/** Applies the next edit to the confirmed queue; ours, if it is, stops being pending. */
	#confirm(confirmed: Queue, { edit, editId, version }: QueueEditMessage) {
		try {
			confirmed.apply(JSON.stringify(edit));
		} catch {
			// The server applied what the core refuses: this device cannot trust its copy.
			this.#refetch = true;
			this.#drain();
			return;
		}
		this.#version = version;
		this.#pending = this.#pending.filter((pending) => pending.editId !== editId);
		this.#rebuild();
	}

	/**
	 * Makes the view from the confirmed queue and the pending edits. A pending edit the queue
	 * no longer accepts, because of what arrived, is dropped and reported as `anchor_gone`: keys
	 * are unique, so only the item it names can have gone, and the user is told. With none pending
	 * the view is the confirmed queue itself, so nothing is copied.
	 */
	#rebuild() {
		const confirmed = this.#confirmed;
		if (!confirmed) return;
		let view = confirmed;
		if (this.#pending.length > 0) {
			view = confirmed.copy();
			this.#pending = this.#pending.filter((pending) => {
				try {
					view.apply(JSON.stringify(pending.edit));
					return true;
				} catch (error) {
					this.#refused({ code: 'anchor_gone', message: String(error), edit: pending.edit });
					return false;
				}
			});
		}
		if (this.#view !== confirmed) this.#view?.free();
		this.#view = view;
		this.#emit();
	}

	/** Lets go of both queues' memory. */
	#release() {
		if (this.#view !== this.#confirmed) this.#view?.free();
		this.#confirmed?.free();
		this.#confirmed = this.#view = null;
	}

	/** Tells the page what changed. */
	#emit() {
		const view = this.#view;
		this.#changed({
			queueId: this.#queueId,
			context: this.#context,
			version: this.#version,
			revision: ++this.#revision,
			current: view?.current() ?? null,
			manual: view?.manualLen() ?? 0,
			upcoming: view?.upcomingLen() ?? 0,
			history: view?.historyLen() ?? 0,
			repeat: (view?.repeat() ?? 'off') as Repeat,
			shuffled: view?.shuffled() ?? false,
			settling: this.#placeholder,
			starting: this.#launch !== null
		});
	}

	/** Sends and fetches what is waiting, one request at a time, so the server hears edits in order. */
	async #drain() {
		if (this.#draining) return;
		this.#draining = true;
		try {
			while (await this.#step());
		} finally {
			this.#draining = false;
		}
	}

	/**
	 * Does the next thing waiting: a start, then edits, then a fetch. False when none is. Edits
	 * go before the fetch so each is answered, or known unsent, by the time the snapshot is
	 * installed; one the server already has, whose answer was lost, is not applied twice.
	 */
	async #step(): Promise<boolean> {
		const next = this.#pending.find((pending) => pending.version === undefined);
		if (this.#launch) await this.#sendStart(this.#launch);
		else if (next) await this.#sendEdit(next);
		else if (this.#refetch) await this.#fetchQueue();
		else return false;
		return true;
	}

	/** One request to the API; null when it could not be made, or the server was too busy or broken. */
	async #call(
		method: 'GET' | 'POST',
		path: string,
		body?: object,
		extra: Record<string, string> = {}
	): Promise<Response | null> {
		const headers: Record<string, string> = { 'X-Jewelcase-Client': 'web', ...extra };
		if (body) headers['Content-Type'] = 'application/json';
		const response = await this.#fetch(`${this.#api}${path}`, {
			method,
			headers,
			body: body && JSON.stringify(body)
		}).catch(() => null);
		if (response && response.status < 500 && response.status !== 429) return response;
		await new Promise((resolve) => setTimeout(resolve, this.#retryMs));
		return null;
	}

	/** Sends the start; sent again, by its `queueId`, if the answer was lost. */
	async #sendStart(launch: StartQueue) {
		const response = await this.#call('POST', '/me/queue', launch);
		if (!response || this.#launch !== launch) return;
		this.#launch = null;
		this.#emit();
		// What the device holds of a query's queue is its first track: the server's is the queue.
		if (response.ok && 'query' in launch.source) this.#refetch = true;
		if (!response.ok) {
			// What the device shows is not the server's queue: fetch that.
			this.#refetch = true;
			this.#refused(await refusal(response));
		}
	}

	/** Sends one edit, again with the same `editId` if the answer was lost. */
	async #sendEdit(pending: Pending) {
		const response = await this.#call('POST', '/me/queue/edits', {
			editId: pending.editId,
			queueId: pending.queueId,
			edit: pending.edit
		});
		if (!response || !this.#pending.includes(pending)) return;
		if (response.ok) {
			pending.version = ((await response.json()) as { version: number }).version;
			// Its `queueEdit` came before the answer, or was lost with the connection.
			const confirmed = this.#queueId === pending.queueId && this.#version >= pending.version;
			if (confirmed) this.#retire(pending);
			return;
		}
		const problem = await refusal(response);
		this.#retire(pending);
		if (problem.code === 'queue_changed' || response.status === 404) this.#refetch = true;
		// A queue that is no longer the account's takes its edits with it, without comment.
		if (problem.code !== 'queue_changed') this.#refused({ ...problem, edit: pending.edit });
	}

	/** Stops the edit being pending, so the view is made without it. */
	#retire(pending: Pending) {
		this.#pending = this.#pending.filter((other) => other !== pending);
		this.#rebuild();
	}

	/**
	 * Loads the server's queue, and puts the pending edits back on top. The request names the
	 * queue and version held, so the server answers `304`, with nothing to download, if that is
	 * still its queue.
	 */
	async #fetchQueue() {
		this.#refetch = false;
		this.#held = [];
		const known = this.#fromServer && this.#run !== null && this.#queueId;
		const response = await this.#call(
			'GET',
			'/me/queue',
			undefined,
			known ? { 'If-None-Match': `"${this.#run}.${known}.${this.#version}"` } : {}
		);
		const held = this.#held;
		this.#held = null;
		// A start since then is newer than what was fetched, and must reach the server first.
		if (!response || this.#launch) {
			this.#refetch = true;
			return;
		}
		this.#run = runOf(response.headers.get('ETag'));
		if (response.status === 404) {
			this.#clear();
		} else if (response.status === 304) {
			// The server is where this device is, so an edit it answered but this device never
			// heard confirmed is one the server has lost.
			this.#keepUnanswered();
			for (const message of held) this.#heard(message);
		} else if (response.ok) {
			this.#install(Fetched.parse(await response.text()));
			for (const message of held) this.#heard(message);
		} else {
			this.#refetch = true;
			await new Promise((resolve) => setTimeout(resolve, this.#retryMs));
		}
	}

	/** The account has no queue. */
	#clear() {
		this.#placeholder = false;
		this.#release();
		this.#queueId = null;
		this.#context = null;
		this.#version = 0;
		this.#pending = [];
		this.#emit();
	}

	/**
	 * Drops the pending edits the server answered. Edits are sent before a fetch, so one it
	 * answered is either in what the fetch shows or was lost by the server, as in a restart;
	 * either way it is not replayed, and sending it again could apply it twice.
	 */
	#keepUnanswered() {
		const kept = this.#pending.filter((pending) => pending.version === undefined);
		if (kept.length === this.#pending.length) return;
		this.#pending = kept;
		this.#rebuild();
	}

	/** Makes the snapshot the confirmed queue; pending edits for another queue go. */
	#install(fetched: Fetched) {
		const { queueId, version } = fetched;
		const context = JSON.parse(fetched.context) as Context;
		const next = fetched.queue();
		this.#placeholder = false;
		this.#release();
		this.#confirmed = this.#view = next;
		this.#queueId = queueId;
		this.#context = context;
		this.#version = version;
		this.#fromServer = true;
		// Of the pending edits, only those for this queue that the server never answered stay.
		this.#pending = this.#pending.filter(
			(pending) => pending.queueId === queueId && pending.version === undefined
		);
		this.#rebuild();
	}
}

/** The reason the server refused a request. */
async function refusal(response: Response): Promise<Refusal> {
	const problem: Problem = await response.json().catch(() => null);
	return {
		code: problem?.code ?? 'failed',
		message: problem?.detail ?? problem?.title ?? response.statusText
	};
}

/** The run part of an `ETag`, `"<run>.<queueId>.<version>"`, or `null` without one. */
function runOf(etag: string | null): string | null {
	return etag?.replace(/^"/, '').split('.')[0] || null;
}
