// Dragging songs, or an album, artist or playlist, from a browse view to the queue panel's Up
// Next (`requirements/queue.md` §3). Only a mouse drags: touch and keyboard add from the menu
// (`requirements/conventions.md` §2), and a finger that moves is scrolling.

import type { Adding } from './adding.svelte';

/** What is being dragged, and its library, while a drag from a browse view is under way. */
export const carried: { what: Adding | null; libraryId: string } = { what: null, libraryId: '' };

/** Whether the main pointer is a mouse, so rows and cards can be dragged. */
export const canDrag = typeof matchMedia === 'function' && matchMedia('(pointer: fine)').matches;

/** Starts dragging `what`, from a `dragstart`; `dragend` on the same element calls [`drop`]. */
export function lift(e: DragEvent, what: Adding, libraryId: string) {
	// Firefox does not start a drag that carries nothing.
	e.dataTransfer?.setData('text/plain', what.label);
	if (e.dataTransfer) e.dataTransfer.effectAllowed = 'copy';
	carried.what = what;
	carried.libraryId = libraryId;
}

/** Lets go of what was carried, once the drag has ended or been dropped. */
export function drop() {
	carried.what = null;
}

/** How close to an edge of the scrolling box, in pixels, a drag starts to scroll it. */
const EDGE = 56;

/**
 * How far to scroll for a drag at `y` (page coordinates) over `box`: toward the edge it is
 * near, faster the closer it gets, at most 20 pixels a frame; 0 when it is away from both.
 */
export function edgeStep(y: number, box: { top: number; bottom: number }): number {
	const into =
		y < box.top + EDGE ? y - (box.top + EDGE) : y > box.bottom - EDGE ? y - (box.bottom - EDGE) : 0;
	return Math.max(-20, Math.min(20, into / 3));
}
