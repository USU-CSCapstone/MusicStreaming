import { afterEach, describe, expect, it } from 'vitest';
import { fullText } from './full-text';

/** A box of `width` that cuts off `text` with an ellipsis, as the app's columns do. */
function box(text: string, width: number, title?: string): HTMLElement {
	const el = document.createElement('span');
	Object.assign(el.style, {
		display: 'block',
		width: `${width}px`,
		overflow: 'hidden',
		textOverflow: 'ellipsis',
		whiteSpace: 'nowrap'
	});
	el.innerHTML = text;
	if (title) el.title = title;
	document.body.append(el);
	return el;
}

const hover = (el: HTMLElement) => el.dispatchEvent(new PointerEvent('pointerenter'));

afterEach(() => document.body.replaceChildren());

describe('fullText', () => {
	it('shows the whole text, as one line, while it is cut off', () => {
		const el = box('Welcome to <a href="#">the   Black</a> Parade', 60);
		fullText(el);
		hover(el);
		expect(el.title).toBe('Welcome to the Black Parade');
	});

	it('shows nothing once the text fits, as the width changes', () => {
		const el = box('Roxanne', 40);
		fullText(el);
		hover(el);
		expect(el.title).toBe('Roxanne');
		el.style.width = '400px';
		hover(el);
		expect(el.hasAttribute('title')).toBe(false);
	});

	it('keeps a title of its own while the text fits', () => {
		const el = box('Playback failed', 400, 'The file could not be read');
		fullText(el);
		hover(el);
		expect(el.title).toBe('The file could not be read');
	});

	it('puts the text cut off inside a cell on the whole cell', () => {
		const cell = document.createElement('div');
		Object.assign(cell.style, { display: 'flex', height: '56px', alignItems: 'center' });
		const inner = box('Brandy (You’re a Fine Girl)', 60);
		cell.append(inner);
		document.body.append(cell);
		fullText(cell);
		hover(cell);
		expect(cell.title).toBe('Brandy (You’re a Fine Girl)');
	});

	it('stops when detached', () => {
		const el = box('A Day In The Life', 30);
		fullText(el)();
		hover(el);
		expect(el.hasAttribute('title')).toBe(false);
	});
});
