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
