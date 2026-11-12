// The log and its sender (`design/web.md`, The sync framework): every write the app makes is a
// mutation (`mutation.ts`), kept in a database of the user's own before it is sent, so closing the
// app loses none of them (`requirements/offline.md` §8), and changed on the device at once
// (`requirements/general.md` §3.4).
//
// Everything lives in memory as well, and the database only makes it last: a write that fails is
// told once and the mutation is sent anyway. Each mutation belongs to the tab instance that made
// it, which holds the Web Lock `jewelcase-tab-<instance>` while it lives. A tab sends its own as
// soon as they are written; one tab at a time, under the lock `jewelcase-outbox-<user>`, claims
// those whose owner's lock is free, which a closed tab leaves behind. A claim is one transaction
// that only one tab can win, so no mutation is sent by two. Scopes are sent side by side, so a
// playlist that will not go through never holds up the plays; within a scope, one at a time in
// order, each given what the answer to the one before carried.
//
// Nothing is sent while the device is offline or working offline (`connectivity`). An answered
// failure that may pass is tried again after a pause that grows to `MAX_RETRY_MS`, unless the
// server asks for longer; after `MAX_TRIES` such answers, or `MAX_AGE_MS`, the mutation is set
// aside in `dead`. One logged with a `notBefore` time is held until then, and may be cancelled
// while it is unsent, which takes its change back off the device. A refusal drops the mutation and
// every later one of its scope made on it, says so once, and takes their changes back off the
// device. A 401 stops everything until the session may be good again. What is set aside is listed
// (`setAside`) and can be put back in the log (`retry`) or let go of (`discard`).

import { isRedirect } from '@sveltejs/kit';
import { ApiError } from '$lib/api/client';
import { connectivity } from '$lib/connectivity.svelte';
import { toast } from '$lib/toast.svelte';
import { catalogDevice, type Device } from './device';
import type { Batch, Fetch, Local, Mutation } from './mutation';
import { type Args, type Name, definition } from './mutations';
import {
	type Env,
	type Port,
	adoptLock,
	browserEnv,
	closed,
	holdTab,
	unsent,
	wipeChannel
} from './store';

/** Answered failures a mutation is tried through before it is set aside. */
export const MAX_TRIES = 20;
/** How long a mutation is tried before it is set aside. */
export const MAX_AGE_MS = 7 * 24 * 3600_000;
/** The longest wait between attempts, unless the server asks for more. */
export const MAX_RETRY_MS = 30_000;
/** How long a tab that found another adopting waits to try again. */
const LOCK_RETRY_MS = 2000;

/** What became of a mutation that left the log: sent; refused or set aside; or cancelled. */
export type Settled = 'sent' | 'dropped' | 'cancelled';

/** What became of a mutation: as `Settled`, or still in the log, for another tab. */
export type Outcome = Settled | 'kept';

/**
 * What a mutation that left the log left behind: the answer to one sent, as its definition's `send`
 * resolved; or for one dropped, the server's problem code if it refused it, what the user was told,
 * and whether it was set aside rather than refused.
 */
export type Result = { answer?: unknown; code?: string; message?: string; dead?: boolean };

/**
 * A mutation set aside in `dead`: as the log held it, when it was set aside, why in plain words,
 * and, for one set aside because it was made on another, that other's key (`with`).
 */
export type SetAside = Mutation & { setAsideAt: number; why: string; with?: string };

/** How a mutation that was not sent is dealt with. */
type Verdict = 'halt' | 'retry' | 'drop';

/**
 * A mutation waiting to be sent: when it is due, how often it failed since this page opened, and
 * whether a request of it is under way.
 */
type Entry = { rec: Mutation; due: number; fails?: number; busy?: boolean };

/** What the sender is started with. */
export type Options = {
	env?: Env;
	fetch?: Fetch;
	/** Where the changes mutations make on the device are made; the catalog worker by default. */
	device?: Device;
};

/** A fresh random ID. */
const id = () => crypto.randomUUID();

/** What an answer, or the lack of one, means for the mutation that got it. */
function verdict(error: unknown): Verdict {
	if (isRedirect(error) || (error instanceof ApiError && error.status === 401)) return 'halt';
	if (error instanceof ApiError) {
		const { status } = error;
		return status === 408 || status === 429 || status >= 500 ? 'retry' : 'drop';
	}
	return 'retry';
}

/**
 * The pause before another attempt: as long as the server asked, else growing from 1 s to
 * `MAX_RETRY_MS`, spread so devices do not all return at once.
 */
function backoff(error: unknown, fails: number): number {
	if (error instanceof ApiError && error.retryAfter) return error.retryAfter * 1000;
	return Math.min(MAX_RETRY_MS, 1000 * 2 ** fails) * (0.5 + Math.random() / 2);
}

/**
 * `local`, or a fresh one for the library of `writes`, with `writes`' calls added, each named under
 * `key` and stamped `at`. `local` unchanged where there are no writes.
 */
function stamp(
	key: string,
	local: Local | undefined,
	writes: { library: string; calls: { name: string; args: object }[] } | undefined,
	at: number
): Local | undefined {
	if (!writes || writes.calls.length === 0) return local;
	const calls = local?.calls ?? [];
	const added = writes.calls.map((call, i) => ({ ...call, id: `${key}/${calls.length + i}`, at }));
	return { library: writes.library, calls: [...calls, ...added] };
}

/**
 * This page's sender for the signed-in user: it keeps the mutations given to it in memory and in
 * the user's database, makes their changes on the device, and sends them as soon as it can.
 */
export class Sender {
	#env!: Env;
	#user: string | null = null;
	#instance = '';
	#port: Port | null = null;
	#fetch: Fetch = (...args) => fetch(...args);
	#device: Device = catalogDevice;

	/** The mutations waiting, by scope, each list in the order they are sent. */
	#scopes = new Map<string, Entry[]>();
	/** Who waits for what becomes of each mutation, by key. */
	#waiting = new Map<string, ((outcome: Outcome) => void)[]>();
	#listeners = new Set<(rec: Mutation, outcome: Settled, result: Result) => void>();
	/** The last sequence number given, so this tab's are in the order they were made. */
	#seq = 0;

	/** Every write, in order, so none overtakes another. */
	#tail: Promise<void> = Promise.resolve();
	/** The ports whose database is being deleted, to which no queued write goes. */
	#wiped = new WeakSet<object>();
	/** Whether the user has been told that this browser would not keep what was given to it. */
	#told = false;

	/** Set by a 401, until something says the session may be good again. */
	#halted = false;
	#drainer: Promise<void> | null = null;
	/**
	 * Whether the drain is in its loop, which a kick wakes; else it is ending, and a kick asks for
	 * another.
	 */
	#looping = false;
	#again = false;
	/** Whether the next drain looks for mutations a closed tab left. */
	#adoptDue = true;
	/** How long until another try at the lock, after a tab found another adopting. */
	#retryLock = 0;
	/**
	 * Counts the times the log was let go, so a request that outlives its user's log is not written
	 * into the next.
	 */
	#epoch = 0;
	#wake: (() => void) | null = null;
	/** Counts the times something said the server may be back, so an attempt it overtook goes again. */
	#wakes = 0;
	#timer: ReturnType<typeof setTimeout> | undefined;
	#releaseTab: (() => void) | null = null;
	#stop: (() => void) | null = null;

	/** The `fetch` mutations are sent with. */
	get fetch(): Fetch {
		return this.#fetch;
	}

	/**
	 * Opens `user`'s log and drains it. Does nothing for the user it is open for already; another
	 * user's log is let go first.
	 */
	start(user: string, { env = browserEnv(), fetch: fetcher, device }: Options = {}) {
		if (this.#user === user) return;
		this.stop();
		this.#env = env;
		this.#user = user;
		this.#halted = false;
		this.#instance = id();
		this.#adoptDue = true;
		if (fetcher) this.#fetch = fetcher;
		if (device) this.#device = device;
		this.#port = env.open(user);
		this.#releaseTab = holdTab(env, this.#instance);

		const channel = wipeChannel(user);
		if (channel) channel.onmessage = () => this.#let(true);
		const wake = () => this.wake();
		const visible = () => document.visibilityState === 'visible' && this.wake();
		globalThis.addEventListener?.('online', wake);
		globalThis.document?.addEventListener('visibilitychange', visible);
		const resumed = connectivity.onResume(wake);
		this.#stop = () => {
			channel?.close();
			resumed();
			globalThis.removeEventListener?.('online', wake);
			globalThis.document?.removeEventListener('visibilitychange', visible);
		};
		this.wake();
	}

	/**
	 * Lets go of the log: what it holds stays in the database, for another instance to claim. The
	 * tab's lock is released, so it counts as closed.
	 */
	stop() {
		this.#let(false);
	}

	/** Stops, and with `wiped` drops every write still to come, as the database is being deleted. */
	#let(wiped: boolean) {
		this.#epoch++;
		clearTimeout(this.#timer);
		this.#stop?.();
		this.#stop = null;
		this.#releaseTab?.();
		this.#releaseTab = null;
		const port = this.#port;
		if (port) {
			if (wiped) this.#wiped.add(port);
			void this.#tail.then(() => port.close());
		}
		this.#port = null;
		this.#user = null;
		this.#scopes.clear();
		for (const waiting of this.#waiting.values()) for (const done of waiting) done('kept');
		this.#waiting.clear();
	}

	/**
	 * Keeps the mutation `name` with `args` to send, makes its change on the device, and sends it
	 * now unless `now` is false (a page closing sends it its own way). With `notBefore`, a time in
	 * milliseconds, it is held until then, and `cancel` takes it back meanwhile. An unsent mutation
	 * of the same scope it folds into takes it in. Returns its key, and when the device has its
	 * change (or found it cannot make it); nothing where no log is open.
	 */
	enqueue<N extends Name>(
		name: N,
		args: Args<N>,
		{ now = true, notBefore }: { now?: boolean; notBefore?: number } = {}
	): { key: string; applied: Promise<void> } | undefined {
		const user = this.#user;
		if (!user) return undefined;
		const def = definition(name)!;
		const scope = def.scope(args);
		const list = this.#scopes.get(scope) ?? [];
		const at = Date.now();
		const writes = def.apply?.(args);
		let entry: Entry | undefined;
		// Newest first. One being sent takes it too, and goes again once its request is answered.
		for (let i = list.length - 1; i >= 0 && def.merge && !entry; i--) {
			const merged = def.merge(list[i].rec.args, args);
			if (merged === undefined) continue;
			entry = list[i];
			const { rec } = entry;
			rec.args = merged;
			rec.idem = id();
			rec.local = stamp(rec.key, rec.local, writes, at);
			entry.due = 0;
		}
		if (!entry) {
			const key = id();
			entry = {
				due: notBefore ?? 0,
				rec: {
					key,
					rev: '',
					owner: this.#instance,
					createdAt: at,
					tries: 0,
					name,
					args,
					scope,
					seq: (this.#seq = Math.max(this.#seq + 1, at * 1000)),
					idem: id(),
					local: stamp(key, undefined, writes, at),
					notBefore
				}
			};
			this.#scopes.set(scope, [...list, entry]);
		}
		const { rec } = entry;
		rec.rev = id();
		// Written before the device is asked, which may read the log to apply it again.
		const written = this.#save(rec, true);
		const local = rec.local;
		const applied = local ? written.then(() => this.#apply(user, local)) : written;
		if (now) this.#kick();
		return { key: rec.key, applied };
	}

	/**
	 * Listens to what becomes of each mutation this tab held, with what it left behind; returns how
	 * to stop.
	 */
	onSettled(listener: (rec: Mutation, outcome: Settled, result: Result) => void): () => void {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	}

	/**
	 * Settles with what becomes of the mutation `key`: at once with `kept` where this tab holds no
	 * such mutation.
	 */
	settled(key: string): Promise<Outcome> {
		if (!this.#find(key)) return Promise.resolve('kept');
		return new Promise((resolve) =>
			this.#waiting.set(key, [...(this.#waiting.get(key) ?? []), resolve])
		);
	}

	/**
	 * Takes back the mutation `key` while it is unsent: out of the log, its change taken off the
	 * device, and announced as `cancelled` to who waits for it. Settles with whether it was, once
	 * that is done: false where this tab holds no such mutation, or its request is under way, which
	 * can no longer be called back. Later mutations of its scope are left as they are.
	 */
	async cancel(key: string): Promise<boolean> {
		const found = this.#find(key);
		if (!found || found.entry.busy) return false;
		const { scope, entry } = found;
		const rebased = Promise.resolve().then(() =>
			this.#rebase(entry.rec.local ? [entry.rec.local.library] : [])
		);
		this.#done(scope, entry, 'cancelled', rebased);
		await rebased;
		return true;
	}

	/**
	 * Moves the time the unsent mutation `key` may first be sent to `until` (milliseconds): later to
	 * keep it held, or now to release it. Returns whether this tab holds it unsent; one whose request
	 * is under way cannot be moved.
	 */
	hold(key: string, until: number): boolean {
		const found = this.#find(key);
		if (!found || found.entry.busy) return false;
		const { entry } = found;
		entry.rec.notBefore = until;
		entry.due = until;
		void this.#save(entry.rec);
		this.#kick();
		return true;
	}

	/** The key of the mutation `name` of `scope` this tab holds unsent, if it holds one. */
	unsentOf(name: Name, scope: string): string | undefined {
		return this.#scopes.get(scope)?.find((entry) => entry.rec.name === name)?.rec.key;
	}

	/** The keys of the mutations of `scope` this tab holds unsent, in order. */
	pendingIn(scope: string): string[] {
		return (this.#scopes.get(scope) ?? []).map((entry) => entry.rec.key);
	}

	/**
	 * How many plays and changes the database holds that the server has not confirmed, whichever
	 * tab made them.
	 */
	async pending(): Promise<number> {
		if (!this.#user) return 0;
		await this.#tail;
		try {
			return await unsent(this.#port!);
		} catch {
			return [...this.#scopes.values()].reduce((n, list) => n + list.length, 0);
		}
	}

	/** The mutations set aside, in the order they were made; none where no log is open. */
	async setAside(): Promise<SetAside[]> {
		const port = this.#port;
		if (!port) return [];
		await this.#tail;
		try {
			return (await port.all<SetAside>('dead')).sort((a, b) => a.seq - b.seq);
		} catch (error) {
			this.#failed(error);
			return [];
		}
	}

	/** The set-aside mutation `key` and those set aside because they were made on it, in order. */
	async #group(key: string) {
		return (await this.setAside()).filter((rec) => rec.key === key || rec.with === key);
	}

	/**
	 * Puts the set-aside mutation `key` back in the log, with those set aside because they were made
	 * on it, after everything the log holds and with fresh tries; has the device make their changes
	 * again, and sends them. Settles with whether there was such a one, once the device has them.
	 */
	async retry(key: string): Promise<boolean> {
		const user = this.#user;
		const group = await this.#group(key);
		if (!user || this.#user !== user || group.length === 0) return false;
		const now = Date.now();
		for (const aside of group) {
			const rec: Mutation = {
				key: aside.key,
				rev: id(),
				owner: this.#instance,
				createdAt: aside.createdAt,
				tries: 0,
				since: now,
				name: aside.name,
				args: aside.args,
				scope: aside.scope,
				seq: (this.#seq = Math.max(this.#seq + 1, now * 1000)),
				idem: aside.idem,
				local: aside.local
			};
			this.#scopes.set(rec.scope, [...(this.#scopes.get(rec.scope) ?? []), { rec, due: 0 }]);
			// In the log before it leaves `dead`, so neither a crash nor another tab loses it.
			void this.#save(rec, true);
			void this.#write((port) => port.remove('dead', rec.key));
		}
		await this.#rebase(group.flatMap((rec) => rec.local?.library ?? []));
		this.#kick();
		return true;
	}

	/**
	 * Lets go of the set-aside mutation `key`, with those set aside because they were made on it.
	 * Their changes were taken off the device as they were set aside, which is made sure of again.
	 * Settles with whether there was such a one.
	 */
	async discard(key: string): Promise<boolean> {
		const group = await this.#group(key);
		if (group.length === 0) return false;
		for (const rec of group) void this.#write((port) => port.remove('dead', rec.key));
		await this.#rebase(group.flatMap((rec) => rec.local?.library ?? []));
		return true;
	}

	/**
	 * Sends everything again now, without waiting out a backoff, as something that stopped it may
	 * have passed: the app starting, the server saying hello, the network or the page coming back,
	 * working offline turned off.
	 */
	wake() {
		this.#halted = false;
		this.#adoptDue = true;
		this.#wakes++;
		for (const list of this.#scopes.values())
			for (const entry of list) entry.due = entry.rec.notBefore ?? 0;
		this.#kick();
	}

	/** Settles once what can be sent has been tried, and every write is made. */
	async idle() {
		for (;;) {
			const drainer = this.#drainer;
			await this.#tail;
			if (!drainer && !this.#drainer) return;
			await (drainer ?? this.#drainer);
		}
	}

	/**
	 * Tries to send everything now, and settles once that is over; what is left is what could not
	 * be sent.
	 */
	async flush() {
		this.wake();
		await this.idle();
	}

	// ── The device ───────────────────────────────────────────────

	/**
	 * Has the device make `local`'s changes it has not made, and with `answered` keep them until
	 * the feed brings them, as the server has them; a failure leaves them unshown.
	 */
	async #apply(user: string, local: Local, answered = false) {
		try {
			await this.#device.apply(user, local, answered);
		} catch (error) {
			console.error(error);
		}
	}

	/**
	 * Has the device take back the changes of the mutations of `libraries` and make those still in
	 * the log again, once what was written before is in the database.
	 */
	async #rebase(libraries: Iterable<string>) {
		const user = this.#user;
		if (!user) return;
		const tail = this.#tail;
		await Promise.all(
			[...new Set(libraries)].map((library) =>
				tail.then(() => this.#device.rebase(user, library)).catch(console.error)
			)
		);
	}

	// ── Persistence ───────────────────────────────────────────────

	/**
	 * Queues `op` on the database after every earlier write; a failure is told once, and the work
	 * goes on from memory. Settles once it is done or failed.
	 */
	#write(op: (port: Port) => Promise<unknown>): Promise<void> {
		const port = this.#port;
		if (!port) return this.#tail;
		this.#tail = this.#tail.then(async () => {
			if (this.#wiped.has(port)) return;
			try {
				await op(port);
			} catch (error) {
				this.#failed(error);
			}
		});
		return this.#tail;
	}

	/** Keeps a copy of `rec`, as it is now; with `strict`, on disk before it settles. */
	#save(rec: Mutation, strict = false) {
		// Through JSON, which sees past the page's reactive proxies that `structuredClone` refuses.
		const copy = JSON.parse(JSON.stringify(rec));
		return this.#write((port) => port.put('mutations', copy, strict));
	}

	/** Tells the user, once, that this browser would not keep what was given to it. */
	#failed(error: unknown) {
		console.error(error);
		if (this.#told) return;
		this.#told = true;
		toast(
			'This browser couldn’t keep your changes for later, so they are sent only while the app is open.'
		);
	}

	// ── Draining ───────────────────────────────────────────────

	/** Whether anything may be sent: a log is open, no 401 has stopped it, and the device is online. */
	#canSend() {
		return !!this.#user && !this.#halted && !connectivity.blocked;
	}

	/** Starts a drain, or has the one under way look again. */
	#kick() {
		if (!this.#canSend()) return;
		if (this.#drainer) {
			if (this.#looping) this.#wake?.();
			else this.#again = true;
			return;
		}
		// Begun at once, so a request leaves in the same task as the click that made it.
		this.#drainer = this.#loop().finally(() => {
			this.#drainer = null;
			const again = this.#again;
			this.#again = false;
			if (again) this.#kick();
			else this.#schedule();
		});
	}

	/**
	 * Looks again when the first mutation that is waiting for its time is due, or when the lock is
	 * to be tried again.
	 */
	#schedule() {
		clearTimeout(this.#timer);
		if (!this.#canSend()) return;
		const now = Date.now();
		const retry = this.#retryLock > 0;
		let at = retry ? now + this.#retryLock : Infinity;
		this.#retryLock = 0;
		for (const list of this.#scopes.values())
			for (const { due } of list) if (due > now) at = Math.min(at, due);
		if (at !== Infinity)
			this.#timer = setTimeout(() => {
				if (retry) this.#adoptDue = true;
				this.#kick();
			}, at - now);
	}

	/** Sends from the scopes that have something due, side by side, until none does. */
	async #loop() {
		this.#looping = true;
		const busy = new Map<string, Promise<void>>();
		for (;;) {
			if (this.#canSend()) {
				// One at a time: a second would find the lock held, and in replacing the first here would
				// end the loop before what the first claims is sent.
				if (this.#adoptDue && !busy.has('adopt')) {
					this.#adoptDue = false;
					busy.set(
						'adopt',
						this.#adopt().finally(() => busy.delete('adopt'))
					);
				}
				const now = Date.now();
				// Nothing new is begun while closed tabs' mutations are adopted: those made before this
				// tab's go first in their scope, as a queue's edits must reach it before the next start.
				for (const [scope, list] of busy.has('adopt') ? [] : this.#scopes) {
					const def = definition(list[0].rec.name);
					// A newer build's mutation is left for a build that knows it.
					if (busy.has(`scope:${scope}`) || !def) continue;
					const ready = def.batch ? list.some((e) => e.due <= now) : list[0].due <= now;
					if (!ready) continue;
					const run = def.batch
						? this.#sendBatched(scope, def.batch)
						: this.#sendOne(scope, list[0]);
					busy.set(
						`scope:${scope}`,
						run.finally(() => busy.delete(`scope:${scope}`))
					);
				}
			}
			if (busy.size === 0) {
				this.#looping = false;
				return;
			}
			await Promise.race([
				...busy.values(),
				new Promise<void>((resolve) => (this.#wake = resolve))
			]);
		}
	}

	/** Whether the tab `owner` is closed: its lock is free, and held no longer than it takes to see. */
	async #orphaned(owner: string): Promise<boolean> {
		if (owner === this.#instance) return false;
		return closed(this.#env, owner);
	}

	/**
	 * Takes over the mutations of closed tabs, to send them as its own: one tab at a time looks,
	 * under the lock `jewelcase-outbox-<user>`, and a tab that finds another looking tries again
	 * soon.
	 */
	async #adopt() {
		const lock = await this.#env.tryLock(adoptLock(this.#user!), () => this.#claim());
		if (!lock.ran) this.#retryLock = LOCK_RETRY_MS;
	}

	/**
	 * Claims each mutation this tab does not hold whose owner is closed, each by a transaction that
	 * only one claimant can win, and has the device make its change, which it skips if made.
	 */
	async #claim() {
		const port = this.#port;
		const user = this.#user;
		if (!port || !user) return;
		try {
			for (const rec of await port.all<Mutation>('mutations')) {
				if (this.#find(rec.key) || !(await this.#orphaned(rec.owner))) continue;
				const claimed = await port.claim<Mutation>('mutations', rec.key, rec.owner, this.#instance);
				if (this.#port !== port) return;
				if (!claimed) continue;
				const list = this.#scopes.get(claimed.scope) ?? [];
				list.push({ rec: claimed, due: claimed.notBefore ?? 0 });
				list.sort((a, b) => a.rec.seq - b.rec.seq);
				this.#scopes.set(claimed.scope, list);
				// One kept by version 2 of the database comes with no change made on the device.
				const def = definition(claimed.name);
				if (claimed.local === undefined && def?.apply) {
					claimed.local = stamp(claimed.key, undefined, def.apply(claimed.args), claimed.createdAt);
					if (claimed.local) void this.#save(claimed);
				}
				if (claimed.local) void this.#apply(user, claimed.local);
			}
		} catch (error) {
			this.#failed(error);
		}
	}

	/** The entry of the mutation `key`, and its scope, if this tab holds it. */
	#find(key: string): { scope: string; entry: Entry } | undefined {
		for (const [scope, list] of this.#scopes) {
			const entry = list.find((e) => e.rec.key === key);
			if (entry) return { scope, entry };
		}
		return undefined;
	}

	/** Sends the first mutation of `scope`, with what the one before it carried. */
	async #sendOne(scope: string, entry: Entry) {
		const { rec } = entry;
		const def = definition(rec.name)!;
		const epoch = this.#epoch;
		const current = () => epoch === this.#epoch && this.#scopes.get(scope)?.[0] === entry;
		const rev = rec.rev;
		const wakes = this.#wakes;
		entry.busy = true;
		try {
			const answer = await def.send(rec.args, {
				fetch: this.#fetch,
				key: rec.idem,
				carried: rec.carried
			});
			if (!current()) return;
			// Another change was folded into it meanwhile, which goes next.
			if (rec.rev !== rev) return;
			this.#done(scope, entry, 'sent', undefined, { answer });
			const user = this.#user;
			if (user && rec.local && def.confirmed) {
				const { library, playlistId, version } = def.confirmed(rec.args, answer);
				// Once its removal from the log is written, which the device reads.
				void this.#tail
					.then(() => this.#device.confirmed(user, library, playlistId, version))
					.catch(console.error);
			}
			const next = this.#scopes.get(scope)?.[0];
			if (next && answer !== undefined) {
				next.rec.carried = answer;
				// Saved, so a reopened app sends the next on the version this one produced.
				void this.#save(next.rec);
			}
		} catch (error) {
			if (epoch !== this.#epoch) return;
			this.#failure(scope, entry, error, rev, wakes !== this.#wakes);
		} finally {
			entry.busy = false;
		}
	}

	/**
	 * Sends the mutations of `scope` that are due, oldest first, as many in each request as `batch`,
	 * their definition's, says.
	 */
	async #sendBatched(scope: string, batch: Batch) {
		for (;;) {
			const now = Date.now();
			const due = (this.#scopes.get(scope) ?? [])
				.filter((entry) => entry.due <= now && !entry.busy)
				.slice(0, batch.size);
			if (due.length === 0 || !this.#canSend()) return;
			await this.#sendBatch(scope, due, batch);
		}
	}

	/**
	 * Sends `entries` in one request of `batch`; a request the server refuses is sent again in
	 * halves, to find the mutation it refuses.
	 */
	async #sendBatch(scope: string, entries: Entry[], batch: Batch) {
		const epoch = this.#epoch;
		const revs = entries.map((entry) => entry.rec.rev);
		for (const entry of entries) entry.busy = true;
		try {
			await batch.send(
				entries.map((entry) => entry.rec.args),
				this.#fetch
			);
			if (epoch !== this.#epoch) return;
			// One replaced meanwhile goes again.
			for (const [i, entry] of entries.entries())
				if (entry.rec.rev === revs[i]) this.#done(scope, entry, 'sent');
		} catch (error) {
			if (epoch !== this.#epoch) return;
			const why = verdict(error);
			if (why === 'drop' && entries.length > 1) {
				for (const entry of entries) entry.busy = false;
				const half = entries.length >> 1;
				await this.#sendBatch(scope, entries.slice(0, half), batch);
				// Nothing more of a log let go of meanwhile, whose plays may be another user's.
				if (epoch !== this.#epoch) return;
				await this.#sendBatch(scope, entries.slice(half), batch);
			} else
				for (const [i, entry] of entries.entries()) this.#failure(scope, entry, error, revs[i]);
		} finally {
			for (const entry of entries) entry.busy = false;
		}
	}

	// ── Outcomes ───────────────────────────────────────────────

	/**
	 * Takes `entry` out of its scope and the log: it was sent, or can never be. The device hears
	 * that one sent was answered. Who waits for it hears once its removal from the log is written,
	 * and once `told` settles: for one given up or cancelled, once the device has taken its change
	 * back. Listeners hear `result` with it.
	 */
	#done(scope: string, entry: Entry, outcome: Settled, told?: Promise<void>, result: Result = {}) {
		const { rec } = entry;
		const list = this.#scopes.get(scope);
		if (list?.includes(entry)) {
			const rest = list.filter((e) => e !== entry);
			if (rest.length) this.#scopes.set(scope, rest);
			else this.#scopes.delete(scope);
		}
		const rev = rec.rev;
		const removed = this.#write((port) => port.remove('mutations', rec.key, rev));
		const user = this.#user;
		if (outcome === 'sent' && user && rec.local) void this.#apply(user, rec.local, true);
		const waiting = this.#waiting.get(rec.key) ?? [];
		this.#waiting.delete(rec.key);
		const tell = () => {
			for (const listener of this.#listeners) {
				try {
					listener(rec, outcome, result);
				} catch (error) {
					console.error(error);
				}
			}
			for (const done of waiting) done(outcome);
		};
		void (told ?? removed).then(tell);
	}

	/**
	 * What follows a mutation that did not go: a 401 stops the drain and keeps everything; a
	 * refusal drops it, with what was made on it; an answer that may pass waits as long as the
	 * server says or longer each time, and one that has waited through `MAX_TRIES` answers or
	 * `MAX_AGE_MS` is set aside in the `dead` store, with what was made on it. One that another
	 * change was folded into since `rev`, which it was sent at, goes again at once: the answer was
	 * to what it said before. So does one `woken` while its request was out, which the wake would
	 * have sent at once.
	 */
	#failure(scope: string, entry: Entry, error: unknown, rev: string, woken = false) {
		const why = verdict(error);
		const { rec } = entry;
		if (why === 'halt') {
			this.#halted = true;
			return;
		}
		// The log it belonged to is gone, or it is already given up.
		if (!this.#scopes.get(scope)?.includes(entry)) return;
		if (rec.rev !== rev) return;
		const def = definition(rec.name)!;
		if (why === 'retry') {
			// An unanswered request says nothing of the mutation, nor does one made after the device
			// went offline, nor does a proxy's gateway answer (502, 503, 504) for a server behind it
			// that is down. Only the mutation's age sets those aside.
			const gateway =
				error instanceof ApiError &&
				(error.status === 502 || error.status === 503 || error.status === 504);
			if (!(error instanceof TypeError) && !gateway && !connectivity.blocked) rec.tries++;
			const old = Date.now() - (rec.since ?? rec.createdAt) > MAX_AGE_MS;
			if (rec.tries >= MAX_TRIES || old) {
				this.#giveUp(
					scope,
					entry,
					`Couldn’t ${def.what(rec.args)}. It kept failing, so it was set aside.`,
					{ dead: true },
					old
						? 'It couldn’t be sent for a week.'
						: `The server failed to take it ${MAX_TRIES} times.`
				);
			} else {
				const pause = backoff(error, (entry.fails = (entry.fails ?? 0) + 1) - 1);
				entry.due = woken ? 0 : Date.now() + pause;
				void this.#save(rec);
			}
			return;
		}
		const code = error instanceof ApiError ? error.code : undefined;
		const detail = error instanceof ApiError ? error.message : '';
		const message =
			def.refused?.(rec.args, code) ?? `Couldn’t ${def.what(rec.args)}. ${detail}`.trim();
		this.#giveUp(scope, entry, message, { code });
	}

	/**
	 * Gives `entry` up, with every later mutation of its scope made on it: takes them out (into
	 * `dead`, if `dead`, with `why` for `entry`), tells the user once unless its definition is
	 * `silent` about a refusal, and has the device take their changes back. Listeners hear `code`,
	 * the server's reason for a refusal, with each.
	 */
	#giveUp(scope: string, entry: Entry, message: string, { dead = false, code }: Result, why = '') {
		const { rec } = entry;
		const gone = [
			entry,
			...(this.#scopes.get(scope) ?? []).filter(
				(later) =>
					later !== entry &&
					later.rec.seq > rec.seq &&
					!!definition(later.rec.name)?.depends?.(later.rec.args, rec.args, rec.name)
			)
		];
		const rebased = Promise.resolve().then(() =>
			this.#rebase(gone.flatMap((given) => given.rec.local?.library ?? []))
		);
		for (const given of gone) {
			if (dead) {
				const copy: SetAside = {
					...JSON.parse(JSON.stringify(given.rec)),
					setAsideAt: Date.now(),
					...(given === entry
						? { why }
						: { why: 'It was made on a change that was set aside.', with: rec.key })
				};
				void this.#write((port) => port.put('dead', copy));
			}
			this.#done(scope, given, 'dropped', rebased, { code, message, dead });
		}
		if (dead || !definition(rec.name)?.silent) toast(message);
	}
}

/** The page's one sender. */
export const sender = new Sender();
