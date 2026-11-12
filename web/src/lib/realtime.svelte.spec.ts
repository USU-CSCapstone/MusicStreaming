import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import { connectivity } from './connectivity.svelte';
import { Realtime } from './realtime.svelte';

/** A WebSocket the test plays the server for. */
class FakeSocket {
	sent: { type: string }[] = [];
	closed = false;
	onmessage: ((event: { data: string }) => void) | null = null;
	onclose: (() => void) | null = null;

	send(data: string) {
		this.sent.push(JSON.parse(data));
	}

	close() {
		this.closed = true;
	}

	receive(message: object) {
		this.onmessage?.({ data: JSON.stringify(message) });
	}

	hello() {
		this.receive({ type: 'hello', deviceId: '7', serverTime: new Date().toISOString() });
	}
}

let sockets: FakeSocket[];
let revoked: ReturnType<typeof vi.fn<() => void>>;
let signedIn: boolean;
let realtime: Realtime;

/** The socket opened most recently. */
const latest = () => sockets[sockets.length - 1];

beforeEach(() => {
	vi.useFakeTimers();
	sockets = [];
	revoked = vi.fn<() => void>();
	signedIn = true;
	realtime = new Realtime({
		url: () => 'ws://test/api/v1/realtime',
		create: () => {
			const socket = new FakeSocket();
			sockets.push(socket);
			return socket as unknown as WebSocket;
		},
		signedIn: async () => signedIn,
		revoked
	});
});

afterEach(() => {
	realtime.stop();
	connectivity.reach();
	vi.useRealTimers();
});

describe('Realtime', () => {
	it('is connected once the server says hello, and passes its messages on', () => {
		const heard: string[] = [];
		realtime.on((message) => heard.push(message.type));
		realtime.start();
		expect(realtime.connected).toBe(false);
		expect(realtime.send({ type: 'ping' })).toBe(false);

		latest().hello();
		expect(realtime.connected).toBe(true);
		expect(realtime.deviceId).toBe('7');
		latest().receive({ type: 'devices', devices: [] });
		expect(heard).toEqual(['hello', 'devices']);
		expect(realtime.send({ type: 'ping' })).toBe(true);
	});

	it('reconnects quietly after a blip', () => {
		realtime.start();
		latest().hello();
		latest().onclose?.();
		expect(realtime.connected).toBe(false);

		vi.advanceTimersByTime(1000);
		expect(sockets).toHaveLength(2);
		latest().hello();
		vi.advanceTimersByTime(10_000);
		expect(realtime.connected).toBe(true);
		expect(connectivity.state).toBe('online');
	});

	it('says it is offline once a lost connection stays lost, and backs off', () => {
		realtime.start();
		latest().hello();
		latest().onclose?.();
		vi.advanceTimersByTime(1000);
		latest().onclose?.();
		vi.advanceTimersByTime(2000);
		latest().onclose?.();
		expect(connectivity.state).toBe('unreachable');
		expect(sockets).toHaveLength(3);
		// It waits longer after each failure.
		vi.advanceTimersByTime(2000);
		expect(sockets).toHaveLength(3);

		vi.advanceTimersByTime(2000);
		latest().hello();
		expect(connectivity.state).toBe('online');
	});

	it('says it is offline when it has not connected within a few seconds of starting, and hello clears it', () => {
		realtime.start();
		vi.advanceTimersByTime(2500);
		expect(connectivity.state).toBe('online');
		vi.advanceTimersByTime(1000);
		expect(connectivity.state).toBe('unreachable');

		// A connection that hangs without failing still counts, and the next hello ends it.
		latest().hello();
		expect(connectivity.state).toBe('online');
	});

	it('flags a connection that never comes once, so an answer from HTTP is not undone by each retry', () => {
		realtime.start();
		latest().onclose?.();
		vi.advanceTimersByTime(3500);
		expect(connectivity.state).toBe('unreachable');
		connectivity.reach();
		for (let i = 0; i < 5; i++) {
			vi.advanceTimersByTime(35_000);
			latest().onclose?.();
			vi.advanceTimersByTime(5_000);
		}
		expect(connectivity.state).toBe('online');
	});

	it('takes any message, a pong included, as the server answering', () => {
		realtime.start();
		latest().hello();
		connectivity.lose();
		latest().receive({ type: 'pong', serverTime: new Date().toISOString() });
		expect(connectivity.state).toBe('online');
	});

	it('pings, and gives up on a server that leaves a ping unanswered', () => {
		realtime.start();
		const first = latest();
		first.hello();
		vi.advanceTimersByTime(15_000);
		expect(first.sent).toEqual([{ type: 'ping' }]);
		first.receive({ type: 'pong', serverTime: new Date().toISOString() });
		vi.advanceTimersByTime(15_000);
		expect(realtime.connected).toBe(true);

		vi.advanceTimersByTime(15_000);
		expect(first.closed).toBe(true);
		expect(realtime.connected).toBe(false);
		vi.advanceTimersByTime(1000);
		expect(sockets).toHaveLength(2);
	});

	it('counts from the first ping still unanswered when back in view', () => {
		realtime.start();
		const first = latest();
		first.hello();
		vi.advanceTimersByTime(15_000);
		vi.advanceTimersByTime(14_000);
		document.dispatchEvent(new Event('visibilitychange'));
		vi.advanceTimersByTime(1000);
		expect(realtime.connected, 'unanswered for 15 s').toBe(false);
		expect(first.sent).toEqual([{ type: 'ping' }]);
	});

	it('stops once the server signs this device out', async () => {
		const heard: string[] = [];
		realtime.on((message) => heard.push(message.type));
		realtime.start();
		latest().hello();
		latest().receive({ type: 'sessionRevoked', reason: 'logged_out' });
		await vi.waitFor(() => expect(revoked).toHaveBeenCalledOnce());
		expect(heard).toEqual(['hello']);

		vi.advanceTimersByTime(60_000);
		expect(sockets).toHaveLength(1);
		expect(connectivity.state).toBe('online');
	});

	it('gives up on a handshake that hangs', () => {
		realtime.start();
		vi.advanceTimersByTime(10_000);
		expect(sockets[0].closed).toBe(true);
		vi.advanceTimersByTime(1000);
		expect(sockets).toHaveLength(2);
	});

	it('signs out when refused because the session ended while it could not hear', async () => {
		realtime.start();
		latest().hello();
		signedIn = false;
		latest().onclose?.();
		vi.advanceTimersByTime(1000);
		expect(revoked).not.toHaveBeenCalled(); // A connection that was open dropped; so far so good.
		latest().onclose?.(); // Then the next is refused.
		await vi.waitFor(() => expect(revoked).toHaveBeenCalledOnce());
		vi.advanceTimersByTime(60_000);
		expect(sockets).toHaveLength(2);
	});

	it('wipes the device, whatever the reason, before it goes to the login page', async () => {
		const order: string[] = [];
		realtime.onSessionEnd = async () => void order.push('wiped');
		revoked.mockImplementation(() => void order.push('login'));
		realtime.start();
		latest().hello();
		latest().receive({ type: 'sessionRevoked', reason: 'credentials_changed' });
		await vi.waitFor(() => expect(revoked).toHaveBeenCalledOnce());
		expect(order).toEqual(['wiped', 'login']);
	});

	it('goes to the login page even if the wipe fails', async () => {
		realtime.onSessionEnd = () => Promise.reject(new Error('no pool'));
		vi.spyOn(console, 'error').mockImplementation(() => {});
		realtime.start();
		latest().hello();
		latest().receive({ type: 'sessionRevoked', reason: 'logged_out' });
		await vi.waitFor(() => expect(revoked).toHaveBeenCalledOnce());
	});

	it('does not wipe the device when the server cannot be reached', async () => {
		const wiped = vi.fn();
		realtime.onSessionEnd = wiped;
		realtime.start();
		latest().hello();
		latest().onclose?.();
		vi.advanceTimersByTime(1000);
		latest().onclose?.(); // Refused, and the API cannot say the session ended.
		await vi.advanceTimersByTimeAsync(60_000);
		expect(wiped).not.toHaveBeenCalled();
		expect(revoked).not.toHaveBeenCalled();
	});

	it('with no network makes no attempt and asks no one whether the session is good', async () => {
		const asked = vi.fn(async () => true);
		realtime = new Realtime({
			url: () => 'ws://test/api/v1/realtime',
			create: () => {
				const socket = new FakeSocket();
				sockets.push(socket);
				return socket as unknown as WebSocket;
			},
			signedIn: asked,
			revoked
		});
		const onLine = vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(true);
		onTestFinished(() => onLine.mockRestore());
		realtime.start();
		latest().hello();
		onLine.mockReturnValue(false);
		latest().onclose?.();
		latest().onclose?.(); // A late close of the one let go changes nothing.
		document.dispatchEvent(new Event('visibilitychange'));
		await vi.advanceTimersByTimeAsync(5 * 60_000);
		expect(sockets).toHaveLength(1);
		expect(asked).not.toHaveBeenCalled();

		// Back online, it tries at once.
		onLine.mockReturnValue(true);
		dispatchEvent(new Event('online'));
		expect(sockets).toHaveLength(2);
		latest().hello();
		expect(realtime.connected).toBe(true);
	});

	it('asks no one whether the session is good when an attempt is refused after the network went', async () => {
		const asked = vi.fn(async () => true);
		realtime = new Realtime({
			url: () => 'ws://test/api/v1/realtime',
			create: () => {
				const socket = new FakeSocket();
				sockets.push(socket);
				return socket as unknown as WebSocket;
			},
			signedIn: asked,
			revoked
		});
		const onLine = vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(true);
		onTestFinished(() => onLine.mockRestore());
		realtime.start();
		// The attempt never said hello: it was refused, which is when the session is asked after.
		onLine.mockReturnValue(false);
		latest().onclose?.();
		await vi.advanceTimersByTimeAsync(5 * 60_000);
		expect(asked).not.toHaveBeenCalled();
		expect(sockets).toHaveLength(1);
	});

	it('reconnects a page in the background once the network is back', async () => {
		const onLine = vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(true);
		const hidden = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
		onTestFinished(() => (onLine.mockRestore(), hidden.mockRestore()));
		realtime.start();
		latest().hello();
		onLine.mockReturnValue(false);
		latest().onclose?.();
		onLine.mockReturnValue(true);
		dispatchEvent(new Event('online'));
		await vi.advanceTimersByTimeAsync(10 * 60_000);
		expect(sockets.length).toBeGreaterThan(1);
	});

	it('started with no network makes no attempt until the browser is back online', () => {
		const onLine = vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(false);
		onTestFinished(() => onLine.mockRestore());
		realtime.start();
		vi.advanceTimersByTime(60_000);
		expect(sockets).toHaveLength(0);
		onLine.mockReturnValue(true);
		dispatchEvent(new Event('online'));
		expect(sockets).toHaveLength(1);
	});

	it('with a network but no server still backs off and asks whether the session is good', async () => {
		const asked = vi.fn(async () => true);
		realtime = new Realtime({
			url: () => 'ws://test/api/v1/realtime',
			create: () => {
				const socket = new FakeSocket();
				sockets.push(socket);
				return socket as unknown as WebSocket;
			},
			signedIn: asked,
			revoked
		});
		realtime.start();
		latest().onclose?.();
		await vi.advanceTimersByTimeAsync(1000);
		expect(sockets).toHaveLength(2);
		expect(asked).toHaveBeenCalledOnce();
	});

	it('reconnects at once when the browser is back online', () => {
		realtime.start();
		latest().hello();
		latest().onclose?.();
		dispatchEvent(new Event('online'));
		expect(sockets).toHaveLength(2);
	});
});
