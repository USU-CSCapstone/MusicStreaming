// Gets a request to the one tab that owns the catalog, from any tab and from a tab's workers, and
// tells every tab what the owner says of the catalog. The asker broadcasts a request with an ID
// and the name of its own reply channel; the owner answers on that channel alone, so no other tab
// hears the answer. A request that has no answer when a new owner announces itself is sent
// again. Plain browser APIs, so it runs in a page and in a worker alike.

import type { Kind } from './kinds';
import { PROTOCOL, type CatalogState, type Request } from './protocol';

/** The channel every tab and worker of the origin talks on. */
export const CHANNEL = 'jewelcase-catalog';
/** How long a request may wait for an answer, from when it was last sent. */
export const TIMEOUT_MS = 10_000;
/** How long the owner keeps a reply channel open after its last answer on it. */
export const REPLY_IDLE_MS = 60_000;

/**
 * What starts a timer, so a test can run time by hand: calls `fn` after `ms`, and returns what
 * cancels it.
 */
export type Clock = { after(ms: number, fn: () => void): () => void };

/** The browser's own timers. */
export const realClock: Clock = {
	after(ms, fn) {
		const timer = setTimeout(fn, ms);
		return () => clearTimeout(timer);
	}
};

/** What goes over a channel; each message also carries the protocol. */
type Wire =
	/** A request for the owner, answered on the channel named `reply`. */
	| { type: 'request'; id: string; reply: string; request: Request }
	/** The owner's answer, sent on the asker's reply channel. */
	| { type: 'answer'; id: string; result?: string; error?: string }
	/** A tab has become the owner and can answer. */
	| { type: 'owner' }
	/** A tab that has just joined asks the owner, once, to say what it holds. */
	| { type: 'who' }
	/**
	 * The owner's state, sent when it changes, when it starts and stops serving, and to a joining
	 * tab.
	 */
	| { type: 'state'; state: CatalogState }
	/** The owner changed the library's rows on the device, in `kinds`. */
	| { type: 'changed'; library: string; kinds: readonly Kind[] };

/** Answers a request with its JSON; never answers if the owner goes away meanwhile. */
export type Handler = (request: Request) => Promise<string>;

/** The error of a request nobody answered in time, which says nothing of what was asked. */
export class Unanswered extends Error {
	constructor() {
		super('the catalog did not answer');
	}
}

/** A request waiting for its answer. */
type Pending = {
	request: Request;
	timeout: number;
	resolve: (result: string) => void;
	reject: (error: Error) => void;
	cancel?: () => void;
};

/** A tab's (or worker's) end of the catalog's channel. */
export class CatalogRouter {
	/**
	 * Whether a message of another protocol has been heard: an owner this tab cannot talk to may be
	 * there.
	 */
	foreign = false;

	#channel: BroadcastChannel;
	/** The channel this end's askers are answered on, and its name, which every request carries. */
	#replies: BroadcastChannel;
	#reply: string;
	/** The channels this end answers others on, until idle. */
	#answering = new Map<string, { channel: BroadcastChannel; cancel: () => void }>();
	#timeout: number;
	#clock: Clock;
	#pending = new Map<string, Pending>();
	#handler: Handler | null = null;
	/** The state this end last published as the owner, which it repeats to a tab that asks. */
	#published: CatalogState | null = null;
	#overheard?: (request: Request) => void;
	#listeners = new Set<(library: string, kinds: readonly Kind[]) => void>();
	#states?: (state: CatalogState, own: boolean) => void;

	/**
	 * Joins the channel `name`; tests use their own name, a short timeout and a clock they run by
	 * hand. `heard` is told of every request on the channel, whether or not this end can answer it.
	 */
	constructor(
		name = CHANNEL,
		timeout = TIMEOUT_MS,
		heard?: (request: Request) => void,
		clock: Clock = realClock
	) {
		this.#timeout = timeout;
		this.#overheard = heard;
		this.#clock = clock;
		this.#channel = new BroadcastChannel(name);
		this.#channel.onmessage = ({ data }: MessageEvent<Wire>) => this.#heard(data);
		this.#reply = `${name}-reply-${crypto.randomUUID()}`;
		this.#replies = new BroadcastChannel(this.#reply);
		this.#replies.onmessage = ({ data }: MessageEvent<Wire>) => this.#heard(data);
	}

	/**
	 * Asks the owner, which may be this tab, and resolves with its answer. Rejects with the
	 * owner's error, or with `Unanswered` if nobody answered in `timeout` milliseconds, the usual
	 * wait unless given. An owner that goes away leaves it waiting for the next one.
	 */
	ask(request: Request, timeout = this.#timeout): Promise<string> {
		return new Promise((resolve, reject) => {
			const id = crypto.randomUUID();
			this.#pending.set(id, { request, timeout, resolve, reject });
			this.#send(id);
		});
	}

	/**
	 * Makes this tab the owner: it answers every request with `handler`, including those
	 * already waiting, until the returned function is called.
	 */
	serve(handler: Handler): () => void {
		this.#handler = handler;
		this.#post(this.#channel, { type: 'owner' });
		for (const id of this.#pending.keys()) this.#send(id);
		return () => {
			if (this.#handler === handler) this.#handler = null;
		};
	}

	/**
	 * Tells every tab, this one included, the owner's state; a tab that joins later is told it
	 * again when it asks.
	 */
	publish(state: CatalogState) {
		this.#published = state;
		this.#post(this.#channel, { type: 'state', state });
		this.#states?.(state, true);
	}

	/**
	 * Calls `listener` with each state the owner publishes, this tab's own included, and whether it
	 * is this tab's own.
	 */
	onState(listener: (state: CatalogState, own: boolean) => void) {
		this.#states = listener;
	}

	/** Asks the owner, if there is one, to publish its state again. */
	who() {
		this.#post(this.#channel, { type: 'who' });
	}

	/** Tells every tab, this one included, that `library`'s rows on the device changed in `kinds`. */
	announce(library: string, kinds: readonly Kind[]) {
		this.#post(this.#channel, { type: 'changed', library, kinds });
		for (const listener of this.#listeners) listener(library, kinds);
	}

	/**
	 * Calls `listener` with each library announced, by any tab, and its kinds, until the returned
	 * function is called.
	 */
	listen(listener: (library: string, kinds: readonly Kind[]) => void): () => void {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	}

	/** Posts `message` on `channel`, with the protocol. */
	#post(channel: BroadcastChannel, message: Wire) {
		channel.postMessage({ ...message, protocol: PROTOCOL });
	}

	/**
	 * Answers on the reply channel `name`, which stays open until it has been idle for
	 * `REPLY_IDLE_MS`.
	 */
	#answerOn(name: string, message: Wire) {
		const open = this.#answering.get(name);
		open?.cancel();
		const channel = open?.channel ?? new BroadcastChannel(name);
		const cancel = this.#clock.after(REPLY_IDLE_MS, () => {
			channel.close();
			this.#answering.delete(name);
		});
		this.#answering.set(name, { channel, cancel });
		this.#post(channel, message);
	}

	/** Sends the pending request `id` to the owner and starts its timeout again. */
	#send(id: string) {
		const entry = this.#pending.get(id);
		if (!entry) return;
		entry.cancel?.();
		entry.cancel = this.#clock.after(entry.timeout, () => this.#settle(id, new Unanswered()));
		if (this.#handler) this.#answer(this.#handler, entry.request, (a) => this.#settle(id, a));
		else
			this.#post(this.#channel, {
				type: 'request',
				id,
				reply: this.#reply,
				request: entry.request
			});
	}

	/**
	 * Settles the pending request `id`, if it still waits, with an answer or with the error that ends
	 * it.
	 */
	#settle(id: string, answer: { result?: string; error?: string } | Unanswered) {
		const entry = this.#pending.get(id);
		if (!entry) return;
		entry.cancel?.();
		this.#pending.delete(id);
		if (answer instanceof Unanswered) entry.reject(answer);
		else if (answer.error !== undefined) entry.reject(new Error(answer.error));
		else entry.resolve(answer.result ?? '');
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

	/**
	 * Handles a message: serves a request, settles an answer, resends on a new owner. One of another
	 * protocol is only noted.
	 */
	#heard(wire: Wire & { protocol?: number }) {
		if (wire.protocol !== PROTOCOL) {
			this.foreign = true;
			return;
		}
		if (wire.type === 'request') {
			this.#overheard?.(wire.request);
			const handler = this.#handler;
			if (handler)
				this.#answer(handler, wire.request, (a) =>
					this.#answerOn(wire.reply, { type: 'answer', id: wire.id, ...a })
				);
		} else if (wire.type === 'answer') {
			this.#settle(wire.id, wire);
		} else if (wire.type === 'state') {
			this.#states?.(wire.state, false);
		} else if (wire.type === 'who') {
			if (this.#handler && this.#published)
				this.#post(this.#channel, { type: 'state', state: this.#published });
		} else if (wire.type === 'changed') {
			for (const listener of this.#listeners) listener(wire.library, wire.kinds);
		} else if (wire.type === 'owner' && !this.#handler) {
			for (const id of this.#pending.keys()) this.#send(id);
		}
	}
}
