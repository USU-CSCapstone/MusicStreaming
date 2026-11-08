import { afterEach, expect, it, vi } from 'vitest';
import { connectivity } from '$lib/connectivity.svelte';
import { checkForUpdate } from './app-update';

afterEach(() => vi.unstubAllGlobals());

it('asks the registered worker to update, and shrugs off a failure or no worker', async () => {
	const update = vi.fn().mockRejectedValue(new Error('offline'));
	const getRegistration = vi.fn().mockResolvedValue({ update });
	vi.stubGlobal('navigator', { serviceWorker: { getRegistration } });
	checkForUpdate();
	await vi.waitFor(() => expect(update).toHaveBeenCalledOnce());

	getRegistration.mockResolvedValue(undefined);
	checkForUpdate();
	await vi.waitFor(() => expect(getRegistration).toHaveBeenCalledTimes(2));

	vi.stubGlobal('navigator', {});
	expect(() => checkForUpdate()).not.toThrow();
});

it('does nothing while working offline', async () => {
	const getRegistration = vi.fn().mockResolvedValue({ update: vi.fn() });
	vi.stubGlobal('navigator', { serviceWorker: { getRegistration } });
	connectivity.workOffline = true;
	try {
		checkForUpdate();
		await Promise.resolve();
		expect(getRegistration).not.toHaveBeenCalled();
	} finally {
		connectivity.workOffline = false;
	}
});
