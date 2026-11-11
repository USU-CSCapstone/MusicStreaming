import { expect, it, vi } from 'vitest';
import { CatalogSync } from './sync';

it('without OPFS the sync does nothing: it asks the server for nothing and reports nothing', async () => {
	vi.spyOn(navigator.storage, 'getDirectory').mockRejectedValue(new Error('SecurityError'));
	const fetch = vi.fn<typeof globalThis.fetch>();
	const report = vi.fn();
	const sync = new CatalogSync({ api: '/api/v1', user: '1', fetch, report });

	await sync.start();
	sync.message({ type: 'hello', deviceId: '1', serverTime: '' });
	sync.message({ type: 'accountChanged', position: 3 });
	sync.message({ type: 'libraryChanged', libraryId: '1', position: 3 });
	await sync.idle();
	await sync.signOut();

	expect(fetch).not.toHaveBeenCalled();
	expect(report).not.toHaveBeenCalled();
	vi.restoreAllMocks();
});
