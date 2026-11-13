// What the queue worker and the page say to each other (`design/web.md`, Queue worker). The
// worker holds the queue; the page sends it what happened and shows what it reports.

import type { PlayContext, RealtimeServerMessage, SentEdit, StartQueue } from '$lib/api/types';
import type { EditArgs, RestoreArgs, StartArgs } from '$lib/sync/mutations/queue';
import type { Result, Settled } from '$lib/sync/sender';

/** How the queue repeats: not at all, all of it, or the item playing. */
export type Repeat = 'off' | 'all' | 'one';

/**
 * The queue as the page shows it, sent whenever it changes. It holds no tracks. Its counts, as
 * its rows, leave out the rows of tracks the user hid (`requirements/conventions.md` §8).
 */
export type Summary = {
	/** Null until there is a queue. */
	queueId: string | null;
	/** What the queue started from, as the server keeps it; null with no queue. */
	context: PlayContext | null;
	/** The server's version of the queue, as far as this device has applied it. */
	version: number;
	/** Counts up with every change, so something read earlier is known to be stale. */
	revision: number;
	/** The item playing now. */
	current: string | null;
	/** The item playing came from the Manual Queue, not the context. */
	playingManual: boolean;
	/**
	 * The item playing in the server's queue, as far as this device has heard: what a report
	 * may name. Null with no queue, and while a start waits for the server's answer.
	 */
	held: string | null;
	/** The rows of the Manual Queue, which is the top of Up Next, and of all of Up Next. */
	manual: number;
	upNext: number;
	history: number;
	repeat: Repeat;
	/** The context's Up Next is in a shuffled order. */
	shuffled: boolean;
};

/** An edit the queue could not keep, so the page can say so (`requirements/queue.md` §7). */
export type Refusal = {
	/**
	 * Why: the server's problem code such as `anchor_gone` or `play_gone`, the core's code for an
	 * edit it refuses, `superseded` when the server had another queue active by the time the edit
	 * reached it, or `lost` when the server answered the edit but no longer has it, as after a
	 * restart.
	 */
	code: string;
	message: string;
	/** Null when a start, not an edit, did not stay. */
	edit: SentEdit | null;
};

/**
 * A start from a view (`design/queue.md` §3): the request as the server takes it, and what the
 * device holds that the server's does not need. A `tracks` source is the whole list; with a
 * `recipe` the source is `total` long and the view loaded its first rows, `loaded`, which
 * fill the rows that read straight from the list until the list or a page does.
 */
export type StartRequest = StartQueue & {
	total?: number;
	loaded?: string[];
	/**
	 * The app counts itself offline, so the device resolves a search recipe (it is left to the server
	 * otherwise).
	 */
	offline?: boolean;
};

/**
 * The answer to restoring a kept queue (`design/queue.md` §7): the queue is now the active one
 * and `position` is where its playback was left, or why it was not restored: `offline` when the app
 * counts itself offline, `unreachable` when the server could not be reached.
 */
export type RestoreResult =
	| { ok: true; position: { item: string; positionMs: number } | null }
	| { ok: false; code: string; message: string };

/**
 * A start, edit or restore of the queue, as the worker gives it to the page's log, which sends it
 * (`$lib/sync/mutations/queue.ts`).
 */
export type QueueMutation =
	| { name: 'startQueue'; args: StartArgs }
	| { name: 'editQueue'; args: EditArgs }
	| { name: 'restoreQueue'; args: RestoreArgs };

/** Which rows a window is of: the item playing, Up Next, or History (newest first). */
export type RowSection = 'current' | 'upNext' | 'history';

/**
 * One row, as the page shows it. `known` has its `track`. `unknown` is one the worker is
 * still learning, from a list or a page on its way, so the page may wait for it. `unavailable`
 * is one it cannot learn, since its list is not there to be had, so the player skips it in
 * place (`design/queue.md` §3).
 */
export type QueueRow = {
	/** `<key>.<position>`: what edits name. */
	item: string;
	libraryId: string;
	status: 'known' | 'unknown' | 'unavailable';
	/** The track ID when `known`, else null. */
	track: string | null;
	/** A History row's play number. */
	play?: number;
};

/** The worker's answer to a window of rows. */
export type RowsWindow = {
	section: RowSection;
	offset: number;
	/** The rows in the whole section; at most one in `current`. */
	total: number;
	/** The `revision` this was read at, so it is known to be stale once the queue changes. */
	revision: number;
	rows: QueueRow[];
};

/** The page to the worker. */
export type ToWorker =
	/** First: where the API is, and the account signed in. */
	| { type: 'init'; api: string; wasm: string; user: string }
	| { type: 'message'; message: RealtimeServerMessage }
	/** Whether the device is working offline, when the worker makes no request; off until said. */
	| { type: 'offline'; value: boolean }
	| { type: 'edit'; edit: SentEdit; length?: number; offline?: boolean }
	/** Starts a queue; answered with `started` carrying the same `id`. */
	| { type: 'start'; id: number; request: StartRequest }
	/**
	 * Restores a kept queue; answered with `restored` carrying the same `id`. `offline`: the app
	 * counts itself offline, so it is answered at once and nothing is restored.
	 */
	| { type: 'restore'; id: number; queueId: string; offline?: boolean }
	/** What became of a start, edit or restore the page's log held, as its sender heard it. */
	| { type: 'settled'; mutation: QueueMutation; outcome: Settled; result: Result }
	/** Asks for up to `limit` rows from `offset`; answered with `rows` carrying the same `id`. */
	| { type: 'rows'; id: number; section: RowSection; offset: number; limit: number }
	/** The user hid track `trackId` of `libraryId`, or unhid it, here or on another device. */
	| { type: 'hidden'; libraryId: string; trackId: string; hidden: boolean }
	/**
	 * Writes the queue to the database now (the page is hidden, or logging out); answered with
	 * `saved` carrying the same `id`, after every `enqueue` the worker posted before.
	 */
	| { type: 'save'; id: number };

/** The worker to the page. */
export type FromWorker =
	| { type: 'state'; summary: Summary }
	| { type: 'refused'; refusal: Refusal }
	/** Whether the start took: the queue is the new one, and its state follows. */
	| { type: 'started'; id: number; ok: boolean }
	| { type: 'restored'; id: number; result: RestoreResult }
	/** The queue's write is done, or failed; answers `save` with the same `id`. */
	| { type: 'saved'; id: number }
	/** A start, edit or restore for the page's log to keep and send. */
	| { type: 'enqueue'; mutation: QueueMutation }
	/**
	 * Something to tell the user that is not about an edit or a start, which playback would answer
	 * to.
	 */
	| { type: 'notice'; message: string }
	| { type: 'rows'; id: number; window: RowsWindow };
