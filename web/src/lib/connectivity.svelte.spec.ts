import { afterEach, describe, expect, it, vi } from 'vitest';
import { Connectivity } from './connectivity.svelte';

afterEach(() => {
	localStorage.clear();
	vi.restoreAllMocks();
});

/** Makes the browser say it is, or is not, on a network, and announces the change as it does. */
function network(onLine: boolean) {
	vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(onLine);
	dispatchEvent(new Event(onLine ? 'online' : 'offline'));
}

describe('Connectivity', () => {
	it('starts online, says nothing, and makes requests', () => {
		const c = new Connectivity();
		expect([c.state, c.label, c.blocked, c.workOffline]).toEqual(['online', '', false, false]);
	});

	it('is unreachable once the server is lost, still tries requests, and is online again once it answers', () => {
		const c = new Connectivity();
		c.lose();
		expect([c.state, c.label, c.blocked]).toEqual(['unreachable', 'Offline', false]);
		c.reach();
		expect([c.state, c.label]).toEqual(['online', '']);
	});

	it('is offline while the browser has no network, which no answer overrides, and recovers with it', () => {
		const c = new Connectivity();
		network(false);
		expect([c.state, c.label, c.blocked]).toEqual(['offline', 'Offline', true]);
		c.lose();
		c.reach();
		expect(c.state).toBe('offline');
		network(true);
		expect(c.state).toBe('online');
	});

	it('is offline, not unreachable, when the network goes while the server was already lost', () => {
		const c = new Connectivity();
		c.lose();
		network(false);
		expect(c.state).toBe('offline');
		network(true);
		expect(c.state).toBe('unreachable');
	});

	it('working offline is the user’s choice, named, and wins over the network and the server', () => {
		const c = new Connectivity();
		c.setWorkOffline(true);
		expect([c.state, c.label, c.blocked]).toEqual(['working-offline', 'Working offline', true]);
		c.lose();
		network(false);
		expect(c.state).toBe('working-offline');
		c.setWorkOffline(false);
		expect(c.state).toBe('offline');
		network(true);
		expect(c.state).toBe('unreachable');
	});

	it('keeps Work offline for this browser, and off removes it', () => {
		new Connectivity().setWorkOffline(true);
		expect(new Connectivity().workOffline).toBe(true);
		new Connectivity().setWorkOffline(false);
		expect(new Connectivity().workOffline).toBe(false);
		expect(localStorage.getItem('jewelcase-work-offline')).toBeNull();
	});

	it('storage that refuses is not an error: the choice lasts for the tab', () => {
		vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
			throw new DOMException('full', 'QuotaExceededError');
		});
		vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
			throw new DOMException('blocked', 'SecurityError');
		});
		const c = new Connectivity();
		expect(c.workOffline).toBe(false);
		c.setWorkOffline(true);
		expect(c.workOffline).toBe(true);
	});

	it('follows another tab turning it on and off', () => {
		const c = new Connectivity();
		localStorage.setItem('jewelcase-work-offline', '1');
		dispatchEvent(new StorageEvent('storage', { key: 'jewelcase-work-offline', newValue: '1' }));
		expect(c.workOffline).toBe(true);
		localStorage.removeItem('jewelcase-work-offline');
		dispatchEvent(new StorageEvent('storage', { key: 'jewelcase-work-offline', newValue: null }));
		expect(c.workOffline).toBe(false);
	});
});
