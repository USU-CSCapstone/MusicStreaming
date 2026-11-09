import { describe, expect, it } from 'vitest';
import { summarize, underWay } from './scans';
import type { Scan } from './types';

function scan(id: string, state: Scan['state'], finishedAt: string | null = null): Scan {
	return {
		id,
		trigger: 'manual',
		state,
		startedAt: null,
		finishedAt,
		filesSeen: 0,
		filesProcessed: 0,
		added: 0,
		updated: 0,
		missing: 0,
		problems: 0,
		currentFolder: null
	};
}

const finished = scan('1', 'completed', '2026-11-09T12:00:00.000Z');

describe('summarize', () => {
	it('splits what is running, what is next, and the last to finish', () => {
		const running = scan('2', 'running');
		const next = scan('3', 'queued');
		expect(summarize([next, running, finished])).toEqual({ running, next, last: finished });
	});

	it('takes the oldest queued scan as next', () => {
		const older = scan('3', 'queued');
		expect(summarize([scan('4', 'queued'), older]).next).toBe(older);
	});

	it('counts a suspended scan as the last to finish', () => {
		const suspended = scan('5', 'suspended', '2026-11-09T13:00:00.000Z');
		expect(summarize([suspended]).last).toBe(suspended);
	});

	it('is empty for a library never scanned', () => {
		expect(summarize([])).toEqual({ running: null, next: null, last: null });
	});
});

describe('underWay', () => {
	it('is true only while a scan is queued or running', () => {
		expect([
			underWay([scan('1', 'queued')]),
			underWay([scan('1', 'running')]),
			underWay([finished]),
			underWay([])
		]).toEqual([true, true, false, false]);
	});
});
