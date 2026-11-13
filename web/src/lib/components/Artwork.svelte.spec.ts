import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { rgbaToThumbHash } from 'thumbhash';

vi.mock('$app/state', () => ({ page: { data: { library: { id: '11' }, me: { id: '5' } } } }));
vi.mock('$lib/api/client', () => ({ imageUrl: () => '/images/9' }));
const decode = vi.hoisted(() => ({ calls: 0 }));
vi.mock('$lib/placeholder', async (original) => {
	const { placeholderUrl } = await original<typeof import('$lib/placeholder')>();
	return {
		placeholderUrl: (hash: string) => {
			decode.calls++;
			return placeholderUrl(hash);
		}
	};
});

const { default: Artwork } = await import('./Artwork.svelte');

/** The base64 ThumbHash of a 4 by 4 red picture, as the server sends it. */
const hash = btoa(
	String.fromCharCode(
		...rgbaToThumbHash(
			4,
			4,
			new Uint8Array(64).map((_, i) => (i % 4 === 0 || i % 4 === 3 ? 255 : 0))
		)
	)
);
const props = { image: { id: '9', placeholder: hash }, seed: '9', size: 40 };

// The test says when the picture arrives: the browser never fetches it.
const src = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, 'src')!;
beforeEach(() => {
	Object.defineProperty(HTMLImageElement.prototype, 'src', {
		configurable: true,
		get: () => '',
		set() {}
	});
	decode.calls = 0;
});
afterEach(() => Object.defineProperty(HTMLImageElement.prototype, 'src', src));

/** The real picture's element, which is not the placeholder's. */
const picture = (container: Element) => container.querySelector('img:not(.thumb)')!;

describe('Artwork', () => {
	it('draws the placeholder until the image loads', async () => {
		const { container } = await render(Artwork, props);

		await vi.waitFor(() => expect(container.querySelector('img.thumb')).not.toBeNull());
		expect((container.querySelector('img.thumb') as HTMLImageElement).outerHTML).toContain(
			'data:image/png;base64,'
		);
		picture(container).dispatchEvent(new Event('load'));
		await vi.waitFor(() => expect(container.querySelector('.placeholder')).toBeNull());
		expect(container.querySelector('img.thumb')).toBeNull();
		expect(picture(container).classList).toContain('loaded');
	});

	it('keeps the placeholder when the image cannot be fetched', async () => {
		const { container } = await render(Artwork, props);

		await vi.waitFor(() => expect(container.querySelector('img.thumb')).not.toBeNull());
		picture(container).dispatchEvent(new Event('error'));
		await vi.waitFor(() => expect(container.querySelectorAll('img')).toHaveLength(1));
		expect(container.querySelector('img.thumb')).not.toBeNull();
	});

	it('draws the stand-in icon when there is no placeholder', async () => {
		const { container } = await render(Artwork, {
			...props,
			image: { id: '11', placeholder: null }
		});

		expect(container.querySelector('.placeholder svg')).not.toBeNull();
		expect(container.querySelector('img.thumb')).toBeNull();
	});

	it('does not decode the placeholder of a row far off screen until it nears', async () => {
		const target = document.body.appendChild(document.createElement('div'));
		target.style.cssText = 'position: fixed; top: 20000px; width: 40px';
		const { container } = await render(Artwork, { props, target });

		await new Promise((resolve) => setTimeout(resolve, 200));
		expect(decode.calls).toBe(0);
		expect(container.querySelector('img.thumb')).toBeNull();

		target.style.top = '0';
		await vi.waitFor(() => expect(container.querySelector('img.thumb')).not.toBeNull());
		expect(decode.calls).toBe(1);
		target.remove();
	});
});
