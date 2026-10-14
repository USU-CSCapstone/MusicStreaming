import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
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
		expect(realtime.offline).toBe(false);
	});

	it('says it is offline once a lost connection stays lost, and backs off', () => {
		realtime.start();
		latest().hello();
		latest().onclose?.();
		vi.advanceTimersByTime(1000);
		latest().onclose?.();
		vi.advanceTimersByTime(2000);
		latest().onclose?.();
		expect(realtime.offline).toBe(true);
		expect(sockets).toHaveLength(3);
		// It waits longer after each failure.
		vi.advanceTimersByTime(2000);
		expect(sockets).toHaveLength(3);

		vi.advanceTimersByTime(2000);
		latest().hello();
		expect(realtime.offline).toBe(false);
	});

	it('never says it is offline if it was never connected', () => {
		realtime.start();
		for (let i = 0; i < 5; i++) {
			latest().onclose?.();
			vi.advanceTimersByTime(30_000);
		}
		expect(realtime.offline).toBe(false);
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

	it('stops once the server signs this device out', () => {
		const heard: string[] = [];
		realtime.on((message) => heard.push(message.type));
		realtime.start();
		latest().hello();
		latest().receive({ type: 'sessionRevoked', reason: 'logged_out' });
		expect(revoked).toHaveBeenCalledOnce();
		expect(heard).toEqual(['hello']);

		vi.advanceTimersByTime(60_000);
		expect(sockets).toHaveLength(1);
		expect(realtime.offline).toBe(false);
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

	it('reconnects at once when the browser is back online', () => {
		realtime.start();
		latest().hello();
		latest().onclose?.();
		dispatchEvent(new Event('online'));
		expect(sockets).toHaveLength(2);
	});
});
