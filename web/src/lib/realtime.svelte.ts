// The realtime connection (`api/openapi.yaml`, Realtime): live state from the server without
// polling or refreshing (`requirements/realtime.md`). One per tab. It reconnects by itself, and
// quietly: only a connection lost for longer than a blip is shown, so nothing stale passes for
// live (§6).

import { API, loginUrl } from './api/client';
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
/** How long a lost connection may take to come back before the app says it is offline. */
const OFFLINE_AFTER_MS = 3_000;
/** Reconnecting waits twice as long after each failure, up to this. */
const MAX_RETRY_MS = 30_000;

type Listener = (message: RealtimeServerMessage) => void;

export type Options = {
	/** Where the connection goes; `/realtime` on this page's host by default. */
	url?: () => string;
	create?: (url: string) => WebSocket;
	/** Whether the session is still good; asked when a connection is refused. */
	signedIn?: () => Promise<boolean>;
	/** What happens once the server signs this device out; the login page by default. */
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

export class Realtime {
	/** Whether the connection is open and the server has said hello. */
	connected = $state(false);
	/** Whether a connection that was open has been lost for longer than a blip. */
	offline = $state(false);
	/** This browser's device, as the server knows it. */
	deviceId = $state<string | null>(null);
	/** Server time less this browser's, in milliseconds, to time what the server reports. */
	clockOffset = 0;

	#url: () => string;
	#create: (url: string) => WebSocket;
	#signedIn: () => Promise<boolean>;
	#revoked: () => void;
	// eslint-disable-next-line svelte/prefer-svelte-reactivity -- who listens is not state to render
	#listeners = new Set<Listener>();
	#socket: WebSocket | null = null;
	#running = false;
	#wasConnected = false;
	#failures = 0;
	/** When the server was last heard from, and when the latest ping went. */
	#heardAt = 0;
	#pingedAt = 0;
	#retry: ReturnType<typeof setTimeout> | undefined;
	#connectTimer: ReturnType<typeof setTimeout> | undefined;
	#offlineTimer: ReturnType<typeof setTimeout> | undefined;
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

	/** Connects, and keeps reconnecting until [`stop`](#stop). */
	start() {
		if (this.#running) return;
		this.#running = true;
		addEventListener('online', this.#reconnectNow);
		document.addEventListener('visibilitychange', this.#reconnectNow);
		this.#open();
	}

	stop() {
		this.#running = false;
		removeEventListener('online', this.#reconnectNow);
		document.removeEventListener('visibilitychange', this.#reconnectNow);
		clearTimeout(this.#retry);
		this.#drop();
		clearTimeout(this.#offlineTimer);
		this.#offlineTimer = undefined;
		this.offline = false;
		this.#wasConnected = false;
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

	#receive(message: RealtimeServerMessage) {
		const now = Date.now();
		this.#heardAt = now;
		if (message.type === 'hello') {
			this.#setClock(Date.parse(message.serverTime) - now);
			this.deviceId = message.deviceId;
			this.connected = true;
			this.#wasConnected = true;
			this.#failures = 0;
			clearTimeout(this.#connectTimer);
			clearTimeout(this.#offlineTimer);
			this.#offlineTimer = undefined;
			this.offline = false;
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
		this.#sendPing();
	}

	#sendPing() {
		this.#pingedAt = Date.now();
		this.send({ type: 'ping' });
	}

	/** The connection is gone: tries again, and says so if it stays gone. */
	#lost() {
		const refused = !this.connected;
		this.#drop();
		if (!this.#running) return;
		if (this.#wasConnected && this.#offlineTimer === undefined) {
			this.#offlineTimer = setTimeout(() => {
				this.#offlineTimer = undefined;
				if (!this.connected) this.offline = true;
			}, OFFLINE_AFTER_MS);
		}
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
		clearTimeout(this.#retry);
		this.#retry = setTimeout(() => this.#open(), wait);
	}

	#signOut() {
		this.stop();
		this.#revoked();
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
	 * the connection looks open, pings it so a dead one is found by the next check.
	 */
	#reconnectNow = () => {
		if (!this.#running || document.visibilityState === 'hidden') return;
		if (this.connected) {
			this.#sendPing();
			return;
		}
		if (this.#socket) return;
		clearTimeout(this.#retry);
		this.#failures = 0;
		this.#open();
	};
}

export const realtime = new Realtime();
