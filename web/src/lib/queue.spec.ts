import { describe, expect, it } from 'vitest';
import { firstPlayable, restartsOnPrevious } from './queue';

const ok = { availability: 'available' as const };
const gone = { availability: 'missing' as const };

describe('firstPlayable', () => {
	it('starts on the chosen track, or the next that can play', () => {
		expect(firstPlayable([ok, ok], 1)).toBe(1);
		expect(firstPlayable([ok, gone, ok], 1)).toBe(2);
		expect(firstPlayable([gone], 0)).toBeNull();
		expect(firstPlayable([ok, gone], 1)).toBeNull();
	});
});

describe('restartsOnPrevious', () => {
	it('restarts a track that is under way', () => {
		expect(restartsOnPrevious(12, 3)).toBe(true);
	});

	it('steps back near the start of a track', () => {
		expect(restartsOnPrevious(1, 1)).toBe(false);
		expect(restartsOnPrevious(3, 1)).toBe(false);
	});

	it('restarts when nothing came before', () => {
		expect(restartsOnPrevious(0, 0)).toBe(true);
	});
});
