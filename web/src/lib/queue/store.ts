// The account's queue on this device: the server's queue as confirmed, plus this device's own
// edits that the server has not yet confirmed, replayed on top (`design/queue.md` §9,
// `requirements/general.md` §3.4). The order is the shared core's, run in WebAssembly, so this
// holds no rule about it, and it holds no tracks: lists and rows are filled in elsewhere. Plain
// TypeScript so it runs in a worker and in tests; `worker.ts` is its shell.

import { Queue } from '$lib/core-wasm/jewelcase_core_wasm.js';
import type {
	PlayContext,
	QueueEdit,
	QueueSnapshot,
	RealtimeServerMessage,
	SentEdit
} from '$lib/api/types';
import type { Refusal, Repeat, Summary } from './protocol';

export type Options = {
	/** The API's base path, such as `/api/v1`. */
	api: string;
	fetch?: typeof fetch;
	/** How long to wait before sending again what could not be sent. */
	retryMs?: number;
	/** Called after every change, with what the page shows. */
	changed: (summary: Summary) => void;
	/** Called when an edit could not be kept. */
	refused: (refusal: Refusal) => void;
};

/** An edit this device made that the server has not confirmed. */
type Pending = {
	editId: string;
	queueId: string;
	edit: SentEdit;
	/** The edit as the core applies it here; null when only the server can (see `localEdit`). */
	local: QueueEdit | null;
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
	#context: PlayContext | null = null;
	#version = 0;
	/** The part of the server's `ETag` that is new each time it starts, as last given. */
	#run: string | null = null;
	#revision = 0;
	#pending: Pending[] = [];
	#refetch = false;
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
	 * Applies `edit` here at once and sends it in the background, once, however many times the
	 * send is tried: it carries a fresh `editId`. An edit the core refuses changes nothing and is
	 * reported. One only the server can resolve, an `add` of a `recipe`, is just sent, and shows
	 * when the server's `queueEdit` for it arrives.
	 */
	edit(edit: SentEdit) {
		if (!this.#view || !this.#queueId) {
			this.#refused({ code: 'no_queue', message: 'There is no queue to change.', edit });
			return;
		}
		const local = localEdit(edit);
		if (local) {
			const next = this.#view === this.#confirmed ? this.#view.copy() : this.#view;
			const code = applyTo(next, local);
			if (code) {
				if (next !== this.#view) next.free();
				this.#refused({ code, message: `The queue refused this change (${code}).`, edit });
				return;
			}
			this.#view = next;
		}
		this.#pending.push({ editId: crypto.randomUUID(), queueId: this.#queueId, edit, local });
		this.#emit();
		this.#drain();
	}

	/** Takes in what the server said over the realtime connection. */
	message(message: RealtimeServerMessage) {
		if (message.type === 'hello') {
			this.#refetch = true;
			this.#drain();
		} else if (message.type === 'queueReplaced') {
			// A fetch under way may answer with the queue this replaces, so it is fetched again.
			if (this.#held || message.queueId !== this.#queueId) {
				this.#refetch = true;
				this.#drain();
			}
		} else if (message.type === 'queueEdit') {
			this.#heard(message);
		}
	}

	/** An edit another device (or this one) made, as the server applied it. */
	#heard(message: QueueEditMessage) {
		if (this.#held) {
			this.#held.push(message);
			return;
		}
		if (message.queueId !== this.#queueId || !this.#confirmed) {
			this.#refetch = true;
		} else if (message.version === this.#version + 1) {
			this.#confirm(this.#confirmed, message);
		} else if (message.version > this.#version) {
			this.#refetch = true;
		}
		this.#drain();
	}

	/**
	 * Applies the next edit to the confirmed queue; ours, if it is, stops being pending, so it
	 * is never replayed over a queue that has it.
	 */
	#confirm(confirmed: Queue, { edit, editId, version }: QueueEditMessage) {
		if (applyTo(confirmed, edit) !== undefined) {
			// The server applied what the core refuses: this device cannot trust its copy.
			this.#refetch = true;
			return;
		}
		this.#version = version;
		this.#pending = this.#pending.filter((pending) => pending.editId !== editId);
		this.#rebuild();
	}

	/**
	 * Makes the view from the confirmed queue and the pending edits. A pending edit the queue
	 * no longer accepts, because of what arrived, is dropped and reported with the core's code,
	 * `anchor_gone` when the item it was placed by went. A pending `add` of a `recipe` stays
	 * pending without showing: the server applies it in the order it heard it, and so do the
	 * edits replayed on top of it once it arrives. With none pending the view is the confirmed
	 * queue itself, so nothing is copied.
	 */
	#rebuild() {
		const confirmed = this.#confirmed;
		if (!confirmed) return;
		let view = confirmed;
		if (this.#pending.length > 0) {
			view = confirmed.copy();
			this.#pending = this.#pending.filter((pending) => {
				const code = pending.local && applyTo(view, pending.local);
				if (code) this.#drop(pending, code, `The queue refused this change (${code}).`);
				return !code;
			});
		}
		if (this.#view !== confirmed) this.#view?.free();
		this.#view = view;
		this.#emit();
	}

	/** Says an edit did not stay. The caller removes it from `#pending`. */
	#drop(pending: Pending, code: string, message: string) {
		this.#refused({ code, message, edit: pending.edit });
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
		const state: { current: string | null; repeat: Repeat; shuffle: unknown } | null = view
			? JSON.parse(view.toJson())
			: null;
		this.#changed({
			queueId: this.#queueId,
			context: this.#context,
			version: this.#version,
			revision: ++this.#revision,
			current: state?.current ?? null,
			manual: view?.manualLen() ?? 0,
			upNext: view?.upNextLen() ?? 0,
			history: view?.historyLen() ?? 0,
			repeat: state?.repeat ?? 'off',
			shuffled: !!state?.shuffle
		});
	}

	/** Sends and fetches what is waiting, one request at a time, so the server hears edits in order. */
	async #drain() {
		if (this.#draining) return;
		this.#draining = true;
		try {
			// Decided and cleared with no wait between, so what arrives meanwhile is never missed.
			for (let work = this.#next(); work; work = this.#next()) await work();
		} finally {
			this.#draining = false;
		}
	}

	/**
	 * The next thing waiting, or null: an edit, then a fetch. Edits go before the fetch so each is
	 * answered, or known unsent, by the time the snapshot is installed.
	 */
	#next(): (() => Promise<void>) | null {
		const edit = this.#pending.find((pending) => pending.version === undefined);
		if (edit) return () => this.#sendEdit(edit);
		return this.#refetch ? () => this.#fetchQueue() : null;
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
			return;
		}
		const problem = await problemOf(response);
		this.#pending = this.#pending.filter((other) => other !== pending);
		const replaced = problem.code === 'queue_changed';
		if (replaced || response.status === 404) this.#refetch = true;
		this.#drop(pending, replaced ? 'superseded' : problem.code, problem.message);
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
		const response = await this.#call(
			'GET',
			'/me/queue',
			undefined,
			this.#run !== null && this.#queueId
				? { 'If-None-Match': `"${this.#run}.${this.#queueId}.${this.#version}"` }
				: {}
		);
		const held = this.#held;
		this.#held = null;
		if (!response) {
			this.#refetch = true;
			return;
		}
		this.#run = runOf(response.headers.get('ETag')) ?? this.#run;
		if (response.status === 404) {
			this.#clear();
		} else if (response.status === 304) {
			if (this.#pending.length > 0 || held.length > 0) this.#caughtUp(held);
		} else if (response.ok) {
			this.#install((await response.json()) as QueueSnapshot);
			this.#caughtUp(held);
		}
	}

	/** The account has no queue; what was pending went with it. */
	#clear() {
		this.#release();
		this.#queueId = null;
		this.#context = null;
		this.#version = 0;
		this.#supersede(() => true);
		this.#emit();
	}

	/** Makes the snapshot the confirmed queue; pending edits for another queue go. */
	#install({ queueId, version, context, state }: QueueSnapshot) {
		const next = Queue.fromJson(JSON.stringify(state));
		this.#release();
		this.#confirmed = this.#view = next;
		this.#queueId = queueId;
		this.#context = context;
		this.#version = version;
		this.#supersede((pending) => pending.queueId !== queueId);
	}

	/** Drops the pending edits `gone` picks, saying a new queue replaced them. */
	#supersede(gone: (pending: Pending) => boolean) {
		this.#pending = this.#pending.filter((pending) => {
			if (!gone(pending)) return true;
			this.#drop(pending, 'superseded', 'A new queue replaced this change.');
			return false;
		});
	}

	/**
	 * Settles the pending edits once the server's queue is held at `#version`, then replays
	 * the edits heard during the fetch and rebuilds the view. An answered edit at or before
	 * the version is in the queue, and is not replayed. One answered beyond what the server now
	 * has, and not heard, was lost by it, as in a restart: it is reported and not sent again,
	 * which could apply it twice. If a gap in versions is still to be fetched, that waits.
	 */
	#caughtUp(held: QueueEditMessage[]) {
		this.#pending = this.#pending.filter(
			(pending) => pending.version === undefined || pending.version > this.#version
		);
		for (const message of held) this.#heard(message);
		if (!this.#refetch) {
			this.#pending = this.#pending.filter((pending) => {
				if (pending.version === undefined) return true;
				this.#drop(pending, 'lost', 'The server no longer has this change.');
				return false;
			});
		}
		this.#rebuild();
	}
}

/** Applies `edit` to `queue`; the core's refusal code if it refuses, `invalid` for a malformed edit. */
function applyTo(queue: Queue, edit: QueueEdit): string | undefined {
	try {
		return queue.apply(JSON.stringify(edit));
	} catch {
		return 'invalid';
	}
}

/**
 * The edit as the core applies it here, or null if only the server can resolve it: an `add` of
 * a `recipe`, or of no tracks. An add of `tracks` has just their count here; the list attaches
 * later, under the hash the server gives it.
 */
function localEdit(edit: SentEdit): QueueEdit | null {
	if (edit.type !== 'add') return edit;
	const { key, libraryId, tracks } = edit.source;
	if (edit.source.recipe || !tracks?.length) return null;
	return { ...edit, source: { key, libraryId, length: tracks.length } };
}

/** The reason the server refused a request. */
async function problemOf(response: Response): Promise<{ code: string; message: string }> {
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
