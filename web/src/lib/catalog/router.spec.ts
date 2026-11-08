import { expect, it, vi } from 'vitest';
import { CatalogRouter } from './router';

it('tells every tab, the announcing one too, which library changed', async () => {
	const name = `changed-${crypto.randomUUID()}`;
	const here = new CatalogRouter(name);
	const there = new CatalogRouter(name);
	const heardHere = vi.fn();
	const heardThere = vi.fn();
	here.listen(heardHere);
	const stop = there.listen(heardThere);

	here.announce('1');
	expect(heardHere).toHaveBeenCalledWith('1');
	await vi.waitFor(() => expect(heardThere).toHaveBeenCalledWith('1'));

	stop();
	here.announce('2');
	await vi.waitFor(() => expect(heardHere).toHaveBeenCalledWith('2'));
	expect(heardThere).toHaveBeenCalledTimes(1);
});
