import { describe, expect, it } from 'vitest';
import { edgeStep } from './dragging';

describe('edgeStep', () => {
	const box = { top: 100, bottom: 500 };

	it('scrolls toward the edge a drag is near, faster the closer, and not in the middle', () => {
		expect(edgeStep(300, box)).toBe(0);
		expect(edgeStep(130, box)).toBeLessThan(0);
		expect(edgeStep(480, box)).toBeGreaterThan(0);
		expect(Math.abs(edgeStep(110, box))).toBeGreaterThan(Math.abs(edgeStep(140, box)));
		expect(edgeStep(-500, box)).toBe(-20);
		expect(edgeStep(2000, box)).toBe(20);
	});
});
