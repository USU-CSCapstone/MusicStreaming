import { describe, expect, it } from 'vitest';
import { id, toKey } from './db';
import { page, paging } from './catalog';

describe('IDs', () => {
	it('round-trips 63-bit IDs past the safe-integer range', () => {
		const raw = 8351490655157992622n;
		expect(id(raw)).toBe('8351490655157992622');
		expect(toKey(id(raw))).toBe(raw);
	});

	it('refuses anything that cannot be an ID', () => {
		expect(toKey('abc')).toBeNull();
		expect(toKey('')).toBeNull();
		expect(toKey('1; DROP TABLE tracks')).toBeNull();
		expect(toKey(null)).toBeNull();
	});
});

describe('paging', () => {
	it('defaults and bounds the limit', () => {
		expect(paging(new URLSearchParams())).toEqual({ limit: 100, offset: 0 });
		expect(paging(new URLSearchParams({ limit: '5000' })).limit).toBe(1000);
		expect(paging(new URLSearchParams({ limit: '0' })).limit).toBe(100);
	});

	it('hands out a cursor that resumes where the page ended', () => {
		const first = page(['a', 'b'], 5, { limit: 2, offset: 0 });
		expect(first.nextCursor).not.toBeNull();
		expect(paging(new URLSearchParams({ cursor: first.nextCursor!, limit: '2' }))).toEqual({
			limit: 2,
			offset: 2
		});
		expect(page(['e'], 5, { limit: 2, offset: 4 }).nextCursor).toBeNull();
	});
});
