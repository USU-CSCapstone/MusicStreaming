import { describe, expect, it } from 'vitest';
import { findings, folder, progress, summarize, underWay, waiting, withPressed } from './scans';
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
		wholeLibrary: true,
		currentFolder: null
	};
}

const finished = scan('1', 'completed', '2026-11-09T12:00:00.000Z');

describe('summarize', () => {
	it('splits what is running, what is queued, and the last to finish', () => {
		const running = scan('2', 'running');
		const next = scan('3', 'queued');
		expect(summarize([next, running, finished])).toEqual({
			running,
			queued: [next],
			last: finished
		});
	});

	it('puts the oldest queued scan first, as it starts next', () => {
		const [older, newer] = [scan('3', 'queued'), scan('4', 'queued')];
		expect(summarize([newer, older]).queued).toEqual([older, newer]);
	});

	it('counts a suspended scan as the last to finish', () => {
		const suspended = scan('5', 'suspended', '2026-11-09T13:00:00.000Z');
		expect(summarize([suspended]).last).toBe(suspended);
	});

	it('is empty for a library never scanned', () => {
		expect(summarize([])).toEqual({ running: null, queued: [], last: null });
	});
});

describe('waiting', () => {
	it('is a queued scan of the whole library, not one of changed files', () => {
		const changed = { ...scan('3', 'queued'), trigger: 'watch' as const, wholeLibrary: false };
		const scheduled = { ...scan('4', 'queued'), trigger: 'scheduled' as const };
		expect([waiting([changed]), waiting([scheduled, changed]), waiting([finished])]).toEqual([
			null,
			scheduled,
			null
		]);
	});

	it('is not a retry of the roots that did not answer, though it keeps its trigger', () => {
		const retry = { ...scan('3', 'queued'), trigger: 'scheduled' as const, wholeLibrary: false };
		expect(waiting([retry])).toBeNull();
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

describe('withPressed', () => {
	it('shows a manual scan queued at once', () => {
		const running = scan('2', 'running');
		const shown = withPressed([running, finished]);
		expect(shown.map((s) => [s.trigger, s.state])).toEqual([
			['manual', 'queued'],
			['manual', 'running'],
			['manual', 'completed']
		]);
		expect(summarize(shown).running).toBe(running);
	});

	it('adds nothing when a scan of the library is already queued, as the press answers with it', () => {
		const queued = [scan('3', 'queued')];
		expect(withPressed(queued)).toBe(queued);
	});

	it('queues one behind a scan of changed files, which covers only their folders', () => {
		const changed = { ...scan('3', 'queued'), trigger: 'watch' as const, wholeLibrary: false };
		expect(withPressed([changed])).toHaveLength(2);
	});

	it('queues one behind a retry of one root', () => {
		const retry = { ...scan('3', 'queued'), trigger: 'scheduled' as const, wholeLibrary: false };
		expect(withPressed([retry])).toHaveLength(2);
	});
});

describe('progress', () => {
	it('says how many files are read of those found so far', () => {
		const running = { ...scan('2', 'running'), filesSeen: 1234, filesProcessed: 1200 };
		expect(progress(running)).toBe(
			`Read ${(1200).toLocaleString()} of ${(1234).toLocaleString()} files found so far`
		);
		expect(progress({ ...running, filesSeen: 1, filesProcessed: 0 })).toBe(
			'Read 0 of 1 file found so far'
		);
	});

	it('is still looking before it finds a file', () => {
		expect(progress(scan('2', 'running'))).toBe('Looking for files');
	});
});

describe('findings', () => {
	it('names only what changed, and the problems', () => {
		const found = { ...finished, added: 3, missing: 2, problems: 1 };
		expect(findings(found)).toBe('3 added · 2 missing · 1 problem');
		expect(findings({ ...finished, updated: 1, problems: 4 })).toBe('1 updated · 4 problems');
	});

	it('is empty when nothing changed', () => {
		expect(findings(finished)).toBe('');
	});
});

describe('folder', () => {
	it('names the root as the top folder', () => {
		const running = scan('2', 'running');
		expect([
			folder({ ...running, currentFolder: 'Artist/Album' }),
			folder({ ...running, currentFolder: '' }),
			folder(running)
		]).toEqual(['Artist/Album', 'the top folder', null]);
	});
});
