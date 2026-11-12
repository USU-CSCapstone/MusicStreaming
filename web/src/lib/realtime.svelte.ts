// The realtime connection (`api/openapi.yaml`, Realtime): live state from the server without
// polling or refreshing (`requirements/realtime.md`). One per tab. It reconnects by itself, and
// quietly: only a connection lost for longer than a blip is shown, so nothing stale passes for
// live (§6).

import { API, loginUrl } from './api/root';
import { connectivity } from './connectivity.svelte';
import type { RealtimeClientMessage, RealtimeServerMessage } from './api/types';

/**
 * How often the connection is checked: a page cannot see the server's own pings, so it sends
 * its own, and a ping still unanswered at the next check means the connection is dead.
 */
const PING_EVERY_MS = 15_000;
/** How long a ping may go unanswered before the connection counts as dead. */
const ANSWER_WITHIN_MS = 5_000;
/** How long a new connection has to say hello before the attempt is given up. */
const CONNECT_WITHIN_MS = 10_000;
/**
 * How long a lost connection may take to come back before the app counts the server unreachable.
 */
const UNREACHABLE_AFTER_MS = 3_000;
/** Reconnecting waits twice as long after each failure, up to this. */
export const MAX_RETRY_MS = 30_000;

type Listener = (message: RealtimeServerMessage) => void;

export type Options = {
	/** Where the connection goes; `/realtime` on this page's host by default. */
	url?: () => string;
	create?: (url: string) => WebSocket;
	/** Whether the session is still good; asked when a connection is refused. */
	signedIn?: () => Promise<boolean>;
	/**
	 * What happens once the server signs this device out and `onSessionEnd` is done; the login page
	 * by default.
	 */
	revoked?: () => void;
};

const defaultUrl = () => {
	const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
	return `${scheme}//${location.host}${API}/realtime`;
};

/** False only when the server says the session is over; a server that is down says nothing. */
const defaultSignedIn = () =>
	fetch(`${API}/me`).then(
		(response) => response.status !== 401,
		() => true
	);

/** Whether the browser may have a network: with none, nothing can reach the server. */
const networked = () => navigator.onLine !== false;

export class Realtime {
	/** Whether the connection is open and the server has said hello. */
	connected = $state(false);
	/** This browser's device, as the server knows it. */
	deviceId = $state<string | null>(null);
	/** Server time less this browser's, in milliseconds, to time what the server reports. */
	clockOffset = 0;
	/**
	 * What the app does, before the login page loads, once the server has ended the session: wipes
	 * the device.
	 */
	onSessionEnd: () => unknown = () => {};

	#url: () => string;
	#create: (url: string) => WebSocket;
	#signedIn: () => Promise<boolean>;
	#revoked: () => void;
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- who listens is not state to render
	#listeners = new Set<Listener>();
	#socket: WebSocket | null = null;
	#running = false;
	#failures = 0;
	/** When the server was last heard from, and when the latest ping went. */
	#heardAt = 0;
	#pingedAt = 0;
	#retry: ReturnType<typeof setTimeout> | undefined;
	#connectTimer: ReturnType<typeof setTimeout> | undefined;
	#unreachableTimer: ReturnType<typeof setTimeout> | undefined;
	#pinging: ReturnType<typeof setInterval> | undefined;

	constructor({
		url = defaultUrl,
		create = (u) => new WebSocket(u),
		signedIn = defaultSignedIn,
		revoked
	}: Options = {}) {
		this.#url = url;
		this.#create = create;
		this.#signedIn = signedIn;
		this.#revoked = revoked ?? (() => location.assign(loginUrl()));
	}

	/**
	 * Connects, and keeps reconnecting until [`stop`](#stop). While the browser has no network it
	 * makes no attempt, and the `online` event starts one at once.
	 */
	start() {
		if (this.#running) return;
		this.#running = true;
		addEventListener('online', this.#reconnectNow);
		document.addEventListener('visibilitychange', this.#reconnectNow);
		if (networked()) this.#open();
		this.#watch();
	}

	stop() {
		this.#running = false;
		removeEventListener('online', this.#reconnectNow);
		document.removeEventListener('visibilitychange', this.#reconnectNow);
		clearTimeout(this.#retry);
		this.#drop();
		clearTimeout(this.#unreachableTimer);
		this.#unreachableTimer = undefined;
		connectivity.reach();
		this.#failures = 0;
	}

	/** Sends `message` if connected, and whether it was. */
	send(message: RealtimeClientMessage): boolean {
		if (!this.connected || !this.#socket) return false;
		this.#socket.send(JSON.stringify(message));
		return true;
	}

	/** Calls `listener` with every message from the server until the returned function is called. */
	on(listener: Listener): () => void {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	}

	#open() {
		const socket = this.#create(this.#url());
		this.#socket = socket;
		socket.onmessage = (event) => {
			let message: RealtimeServerMessage;
			try {
				message = JSON.parse(event.data);
			} catch {
				return;
			}
			if (typeof message === 'object' && message !== null) this.#receive(message);
		};
		// Refused, closed, or dropped: all the same here.
		socket.onclose = () => {
			if (this.#socket === socket) this.#lost();
		};
		// A handshake can hang for minutes, as when the network changes under it.
		this.#connectTimer = setTimeout(() => this.#lost(), CONNECT_WITHIN_MS);
	}

	/**
	 * Handles a server message: any message counts as the server answering, then it is routed by
	 * type.
	 */
	#receive(message: RealtimeServerMessage) {
		const now = Date.now();
		this.#heardAt = now;
		// Any message is the server answering, a pong included.
		connectivity.reach();
		if (message.type === 'hello') {
			this.#setClock(Date.parse(message.serverTime) - now);
			this.deviceId = message.deviceId;
			this.connected = true;
			this.#failures = 0;
			clearTimeout(this.#connectTimer);
			clearTimeout(this.#unreachableTimer);
			this.#unreachableTimer = undefined;
			clearInterval(this.#pinging);
			this.#pinging = setInterval(() => this.#check(), PING_EVERY_MS);
		} else if (message.type === 'pong') {
			// Halfway between asking and hearing is the best guess at when the server answered.
			this.#setClock(Date.parse(message.serverTime) - (this.#pingedAt + now) / 2);
		} else if (message.type === 'sessionRevoked') {
			this.#signOut();
			return;
		}
		for (const listener of this.#listeners) {
			try {
				listener(message);
			} catch (error) {
				// One listener failing must not keep the message from the rest.
				console.error(error);
			}
		}
	}

	#setClock(offset: number) {
		if (Number.isFinite(offset)) this.clockOffset = offset;
	}

	/**
	 * Gives the connection up if the last ping went unanswered, and pings again. Counting from
	 * the ping, not the clock, holds up in a background tab whose timers the browser slows.
	 */
	#check() {
		if (this.#pingedAt > this.#heardAt) {
			if (Date.now() - this.#pingedAt > ANSWER_WITHIN_MS) this.#lost();
			return;
		}
		this.#pingedAt = Date.now();
		this.send({ type: 'ping' });
	}

	/**
	 * Tells connectivity the server is unreachable if no connection is open when the wait is over:
	 * one lost, or one that never came.
	 */
	#watch() {
		if (this.#unreachableTimer !== undefined) return;
		this.#unreachableTimer = setTimeout(() => {
			this.#unreachableTimer = undefined;
			if (!this.connected) connectivity.lose();
		}, UNREACHABLE_AFTER_MS);
	}

	/**
	 * The connection is gone: tries again, and says so if it stays gone. With no network it neither
	 * tries nor asks after the session until the browser is back online.
	 */
	#lost() {
		const refused = !this.connected;
		this.#drop();
		if (!this.#running) return;
		// Once per failure streak, so a connection that never comes back does not flicker the state.
		if (this.#failures === 0) this.#watch();
		clearTimeout(this.#retry);
		if (!networked()) return;
		// A connection refused may be one whose session ended while this device could not hear
		// it, asleep perhaps; the browser does not say why, so the API is asked.
		if (refused) {
			this.#signedIn().then((signedIn) => {
				if (!signedIn && this.#running) this.#signOut();
			});
		}
		// Spread out, so devices that lost the server together do not all return at once.
		const wait = Math.min(MAX_RETRY_MS, 1000 * 2 ** this.#failures) * (0.5 + Math.random() / 2);
		this.#failures++;
		this.#retry = setTimeout(() => this.#open(), wait);
	}

	/** The session is over: stops, lets the app wipe the device, then goes to the login page. */
	#signOut() {
		this.stop();
		void Promise.resolve(this.onSessionEnd())
			.catch(console.error)
			.then(() => this.#revoked());
	}

	/** Lets go of the socket, without waiting for one that may be dead to say it closed. */
	#drop() {
		clearInterval(this.#pinging);
		clearTimeout(this.#connectTimer);
		this.#pingedAt = this.#heardAt = 0;
		const socket = this.#socket;
		this.#socket = null;
		this.connected = false;
		if (socket) {
			socket.onmessage = socket.onclose = null;
			socket.close();
		}
	}

	/**
	 * Back online, or back in view: tries at once instead of waiting out the backoff, or, if
	 * the connection looks open, checks it now so a dead one is found by the next check at the
	 * latest. Does nothing while the browser has no network, nor, but for the network's return,
	 * while the page is hidden.
	 */
	#reconnectNow = (event?: Event) => {
		if (!this.#running || !networked()) return;
		// The network coming back reconnects a page in the background too, as a phone playing in a
		// pocket is; only coming into view waits for the page to be seen.
		if (event?.type !== 'online' && document.visibilityState === 'hidden') return;
		if (this.connected) {
			this.#check();
			return;
		}
		if (this.#socket) return;
		clearTimeout(this.#retry);
		this.#failures = 0;
		this.#open();
	};
}

export const realtime = new Realtime();
