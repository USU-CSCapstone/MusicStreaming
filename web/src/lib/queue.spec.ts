import { describe, expect, it } from 'vitest';
import { firstPlayable, nextPlayable, previousTarget } from './queue';

const ok = { availability: 'available' as const };
const gone = { availability: 'missing' as const };

describe('nextPlayable', () => {
	it('steps forward and back', () => {
		expect(nextPlayable([ok, ok, ok], 0, 1)).toBe(1);
		expect(nextPlayable([ok, ok, ok], 2, -1)).toBe(1);
	});

	it('skips missing tracks in place', () => {
		expect(nextPlayable([ok, gone, gone, ok], 0, 1)).toBe(3);
		expect(nextPlayable([ok, gone, ok], 2, -1)).toBe(0);
	});

	it('returns null at either end', () => {
		expect(nextPlayable([ok, ok], 1, 1)).toBeNull();
		expect(nextPlayable([ok, gone], 0, 1)).toBeNull();
		expect(nextPlayable([ok, ok], 0, -1)).toBeNull();
	});
});

describe('firstPlayable', () => {
	it('starts on the chosen track, or the next that can play', () => {
		expect(firstPlayable([ok, ok], 1)).toBe(1);
		expect(firstPlayable([ok, gone, ok], 1)).toBe(2);
		expect(firstPlayable([gone], 0)).toBeNull();
	});
});

describe('previousTarget', () => {
	it('restarts a track that is under way', () => {
		expect(previousTarget([ok, ok], 1, 12)).toEqual({ restart: true });
	});

	it('steps back near the start of a track', () => {
		expect(previousTarget([ok, ok], 1, 1)).toEqual({ restart: false, index: 0 });
		expect(previousTarget([ok, gone, ok], 2, 0)).toEqual({ restart: false, index: 0 });
	});

	it('restarts the first track', () => {
		expect(previousTarget([ok, ok], 0, 0)).toEqual({ restart: true });
	});
});
