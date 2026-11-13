import { describe, expect, it, vi } from 'vitest';
import { gated } from './gated-fetch';

describe('gated', () => {
	it('passes a request on until blocked, then fails it as with no network, sending nothing', async () => {
		const real = vi.fn(async () => new Response('ok'));
		let blocked = false;
		const f = gated(() => blocked, real);

		expect(await (await f('/a', { method: 'POST' })).text()).toBe('ok');
		expect(real).toHaveBeenCalledWith('/a', { method: 'POST' });

		blocked = true;
		await expect(f('/b')).rejects.toBeInstanceOf(TypeError);
		expect(real).toHaveBeenCalledOnce();

		blocked = false;
		await f('/c');
		expect(real).toHaveBeenCalledTimes(2);
	});
});
