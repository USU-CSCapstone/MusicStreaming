import { afterEach, expect, it, vi } from 'vitest';
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
