// Text cut off with an ellipsis shows whole in its tooltip, so nothing shown in part is out of
// reach (`requirements/conventions.md` §3).

/**
 * An attachment for an element that cuts its text off with an ellipsis, or holds one that does,
 * such as a column's cell: while the text is cut off, its whole text is the element's tooltip, so
 * a pointer anywhere over the element shows it. Whether it is depends on the width it is given,
 * which changes with the window, so it is checked as a pointer comes over it. A title of the
 * element's own stands otherwise.
 */
export function fullText(el: HTMLElement): () => void {
	const own = el.getAttribute('title');
	const check = () => {
		// An inline element has no width of its own, and cuts nothing off.
		const cut = [el, ...el.querySelectorAll<HTMLElement>('*')].find(
			(e) => e.clientWidth > 0 && e.scrollWidth > e.clientWidth
		);
		const text = cut?.textContent?.replace(/\s+/g, ' ').trim();
		if (text) el.title = text;
		else if (own === null) el.removeAttribute('title');
		else el.title = own;
	};
	el.addEventListener('pointerenter', check);
	return () => el.removeEventListener('pointerenter', check);
}
