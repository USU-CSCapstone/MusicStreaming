// Gets a request to the one tab that owns the catalog, from any tab and from a tab's workers:
// the asker broadcasts it with an ID, the owner answers with the same ID, and a request that
// has no answer when a new owner announces itself is sent again. Plain browser APIs, so it
// runs in a page and in a worker alike.

import type { Request } from './protocol';

/** The channel every tab and worker of the origin talks on. */
export const CHANNEL = 'jewelcase-catalog';
/** How long a request may wait for an answer, from when it was last sent. */
export const TIMEOUT_MS = 10_000;

/** What goes over the channel. */
type Wire =
	| { type: 'request'; id: string; request: Request }
	| { type: 'answer'; id: string; result?: string; error?: string }
	/** A tab has become the owner and can answer. */
	| { type: 'owner' };

/** Answers a request with its JSON; never answers if the owner goes away meanwhile. */
export type Handler = (request: Request) => Promise<string>;

/** A request waiting for its answer. */
type Pending = {
	request: Request;
	timeout: number;
	settle: (answer: { result?: string; error?: string }) => void;
	timer?: ReturnType<typeof setTimeout>;
};

/** A tab's (or worker's) end of the catalog's channel. */
export class CatalogRouter {
	#channel: BroadcastChannel;
	#timeout: number;
	#pending = new Map<string, Pending>();
	#handler: Handler | null = null;
	#overheard?: (request: Request) => void;

	/**
	 * Joins the channel `name`; tests use their own name and a short timeout. `heard` is told of
	 * every request on the channel, whether or not this end can answer it.
	 */
	constructor(name = CHANNEL, timeout = TIMEOUT_MS, heard?: (request: Request) => void) {
		this.#timeout = timeout;
		this.#overheard = heard;
		this.#channel = new BroadcastChannel(name);
		this.#channel.onmessage = ({ data }: MessageEvent<Wire>) => this.#heard(data);
	}

	/**
	 * Asks the owner, which may be this tab, and resolves with its answer. Rejects with the
	 * owner's error, or if nobody answered in `timeout` milliseconds. An owner that goes away
	 * leaves it waiting for the next one.
	 */
	ask(request: Request, timeout = this.#timeout): Promise<string> {
		return new Promise((resolve, reject) => {
			const id = crypto.randomUUID();
			this.#pending.set(id, {
				request,
				timeout,
				settle: ({ result, error }) =>
					error === undefined ? resolve(result ?? '') : reject(new Error(error))
			});
			this.#send(id);
		});
	}

	/**
	 * Makes this tab the owner: it answers every request with `handler`, including those
	 * already waiting, until the returned function is called.
	 */
	serve(handler: Handler): () => void {
		this.#handler = handler;
		this.#channel.postMessage({ type: 'owner' } satisfies Wire);
		for (const id of this.#pending.keys()) this.#send(id);
		return () => {
			if (this.#handler === handler) this.#handler = null;
		};
	}

	/** Sends the pending request `id` to the owner and starts its timeout again. */
	#send(id: string) {
		const entry = this.#pending.get(id);
		if (!entry) return;
		clearTimeout(entry.timer);
		entry.timer = setTimeout(
			() => this.#answered(id, { error: 'the catalog did not answer' }),
			entry.timeout
		);
		if (this.#handler) this.#answer(this.#handler, entry.request, (a) => this.#answered(id, a));
		else this.#channel.postMessage({ type: 'request', id, request: entry.request } satisfies Wire);
	}

	/** Settles the pending request `id`, if it still waits. */
	#answered(id: string, answer: { result?: string; error?: string }) {
		const entry = this.#pending.get(id);
		if (!entry) return;
		clearTimeout(entry.timer);
		this.#pending.delete(id);
		entry.settle(answer);
	}

	/** Runs `handler` on `request` and passes what it answers, or its error, to `then`. */
	#answer(
		handler: Handler,
		request: Request,
		then: (answer: { result?: string; error?: string }) => void
	) {
		handler(request).then(
			(result) => then({ result }),
			(error) => then({ error: error instanceof Error ? error.message : String(error) })
		);
	}

	/** Handles a message of the channel: serves a request, settles an answer, resends on a new owner. */
	#heard(wire: Wire) {
		if (wire.type === 'request') {
			this.#overheard?.(wire.request);
			const handler = this.#handler;
			if (handler)
				this.#answer(handler, wire.request, (a) =>
					this.#channel.postMessage({ type: 'answer', id: wire.id, ...a } satisfies Wire)
				);
		} else if (wire.type === 'answer') {
			this.#answered(wire.id, wire);
		} else if (!this.#handler) {
			for (const id of this.#pending.keys()) this.#send(id);
		}
	}
}
