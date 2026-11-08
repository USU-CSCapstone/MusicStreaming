import { afterEach, describe, expect, it, vi } from 'vitest';
import { Connectivity } from './connectivity.svelte';
import { realtime } from './realtime.svelte';

afterEach(() => {
	localStorage.clear();
	realtime.offline = false;
	vi.restoreAllMocks();
});

describe('Connectivity', () => {
	it('starts online, and says nothing', () => {
		const c = new Connectivity();
		expect(c.workOffline).toBe(false);
		expect(c.stale).toBe(false);
		expect(c.label).toBe('');
	});

	it('keeps Work offline for this browser, and off removes it', () => {
		new Connectivity().setWorkOffline(true);
		expect(new Connectivity().workOffline).toBe(true);
		new Connectivity().setWorkOffline(false);
		expect(new Connectivity().workOffline).toBe(false);
		expect(localStorage.getItem('jewelcase-work-offline')).toBeNull();
	});

	it('working offline is stale and named, and a lost connection says only Offline', () => {
		const c = new Connectivity();
		realtime.offline = true;
		expect([c.stale, c.label]).toEqual([true, 'Offline']);
		c.setWorkOffline(true);
		expect([c.stale, c.label]).toEqual([true, 'Working offline']);
		realtime.offline = false;
		expect([c.stale, c.label]).toEqual([true, 'Working offline']);
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
