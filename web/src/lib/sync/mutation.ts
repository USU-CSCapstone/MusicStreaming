// What a mutation is (`design/web.md`, The sync framework): every write the app makes, kept in the
// log until the server has it. A feature declares each of its mutations once, as a `Definition`
// in `mutations/`, and the registry (`mutations/index.ts`) names them; the sender (`sender.ts`)
// keeps and sends them all by the same rules.

import type { Stored } from './store';

/** The signature of `fetch`. */
export type Fetch = typeof fetch;

/**
 * One write the device's catalog applies (`Catalog.applyCall`): its `name` and `args` as
 * `jewelcase_catalog::mutate::apply` takes them, when it was made (`at`, milliseconds), which a
 * replay applies it with again, and an `id` that names this application, so it is applied once.
 */
export type Call = { id: string; name: string; args: object; at: number };

/** What a mutation changes on the device: writes to one library's catalog, in order. */
export type Local = { library: string; calls: Call[] };

/** A mutation as the log keeps it. */
export type Mutation<A = unknown> = Stored & {
	/** The name of its definition in the registry. */
	name: string;
	/** Its arguments, JSON, ids the client minted included. */
	args: A;
	/** Mutations of one scope are sent strictly in order of `seq`; scopes go side by side. */
	scope: string;
	seq: number;
	/**
	 * The idempotency key its request is sent with. It changes when another mutation is merged into
	 * it, as the server would answer the merged request with what it answered the one before.
	 */
	idem: string;
	/** What it changes on the device; none for a mutation of no catalog data, or of none known. */
	local?: Local;
	/** What the answer to the mutation before it in its scope carried, for its own request. */
	carried?: unknown;
	/** When it may first be sent, milliseconds; it is held until then, and may be cancelled. */
	notBefore?: number;
};

/** What a mutation's request is sent with. */
export type SendContext = {
	fetch: Fetch;
	/** The `Idempotency-Key` of the request; one mutation that makes two requests derives a second. */
	key: string;
	/** What the answer to the mutation before it in its scope carried, if one was sent. */
	carried?: unknown;
};

/** The catalog writes a mutation makes on the device, before they are stamped as `Call`s. */
export type Writes = { library: string; calls: { name: string; args: object }[] };

/** How a kind sent many at a time is sent: how many one request carries, and the request. */
export type Batch<A = unknown> = { size: number; send(args: A[], fetch: Fetch): Promise<void> };

/** One kind of mutation, declared once by its feature. */
export type Definition<A> = {
	/** The scope a mutation with `args` is sent in, in order with the rest of it. */
	scope(args: A): string;
	/**
	 * Sends the mutation with `Idempotency-Key: key`. Resolves with what the next mutation of the
	 * scope is given as `carried`, such as the version a playlist edit produced; rejects with the
	 * answer, which the sender judges (`ApiError`, or a `TypeError` with no answer).
	 */
	send(args: A, context: SendContext): Promise<unknown>;
	/** For a kind sent many at a time: how many one request carries, and the request. */
	batch?: Batch<A>;
	/** What it changes on the device's catalog, if anything. */
	apply?(args: A): Writes | undefined;
	/**
	 * Folds `next` into `pending`, an unsent mutation of the same scope, returning the arguments of
	 * the one that replaces both; undefined where they do not fold.
	 */
	merge?(pending: A, next: A): A | undefined;
	/**
	 * Whether `later` of the same scope was made on `refused`, a mutation of the kind `name`, so it
	 * goes with it. None does, unless this says so.
	 */
	depends?(later: A, refused: unknown, name: string): boolean;
	/**
	 * For a write the device may make again over the server's own rows of it, as a replay does when
	 * a feed brings them while it is in the log: the playlist the answer says the write left, which
	 * the device checks itself against once the write has left the log.
	 */
	confirmed?(args: A, answer: unknown): { library: string; playlistId: string; version: number };
	/** What it does, to finish "Couldn’t …" when it fails. */
	what(args: A): string;
	/** What to tell instead of "Couldn’t …" when the server refused it with `code`, if anything. */
	refused?(args: A, code: string | undefined): string | undefined;
	/**
	 * The feature tells the user itself when the server refuses it, from what `Sender.onSettled`
	 * hears, so the sender does not; one set aside is still told.
	 */
	silent?: boolean;
};
