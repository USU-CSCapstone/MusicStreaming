import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlayerState, ProgressReport } from './api/types';
import { Progress, type Snapshot } from './progress';

const here = '1';
const now: Snapshot = {
	itemId: '0',
	libraryId: '11',
	trackId: '41',
	status: 'playing',
	positionMs: 1000,
	volume: 1
};

/** The account's state with `device` playing, or nothing when it is null. */
const state = (device: string | null): PlayerState => ({
	status: device ? 'playing' : 'stopped',
	activeDeviceId: device,
	itemId: '0',
	libraryId: null,
	trackId: null,
	positionMs: 0,
	positionAt: new Date().toISOString(),
	volume: null
});

let sent: ProgressReport[];
let online: boolean;
let progress: Progress;

beforeEach(() => {
	vi.useFakeTimers();
	sent = [];
	online = true;
	progress = new Progress((report) => {
		if (online) sent.push(report);
		return online;
	});
});

afterEach(() => {
	vi.useRealTimers();
});

const starts = () => sent.map((report) => report.start);

describe('Progress', () => {
	it('reports once the user starts playback here, then while this device is named', () => {
		progress.report(now);
		expect(sent).toEqual([]);

		progress.started();
		progress.report(now);
		// Before the server confirms: it takes this connection's reports in turn, so this one
		// arrives after the takeover, as the audio starting does after picking up.
		progress.report(now);
		expect(starts()).toEqual([true, false]);
		expect(progress.heard(here, state(here), true)).toBeNull();
		progress.report(now);
		expect(sent[2]).toEqual({ ...now, start: false });
	});

	it('stops when another device takes over, and reports nothing after', () => {
		progress.started();
		progress.report(now);
		progress.heard(here, state(here), true);
		vi.advanceTimersByTime(1000);

		expect(progress.heard(here, state('2'), true)).toBe('stop');
		progress.report({ ...now, status: 'paused' });
		expect(sent).toHaveLength(1);
	});

	it('takes no notice of another device named before its own takeover is confirmed', () => {
		progress.started();
		progress.report(now);
		expect(progress.heard(here, state('2'), true)).toBeNull();
		progress.report(now);
		expect(starts(), 'still reporting').toEqual([true, false]);
		vi.advanceTimersByTime(5000);
		expect(progress.heard(here, state('2'), true), 'refused, so obeyed').toBe('stop');
		progress.report(now);
		expect(sent).toHaveLength(2);
	});

	it('does not try a refused takeover again when nothing else plays', () => {
		progress.started();
		progress.report(now);
		vi.advanceTimersByTime(5000);
		progress.report(now);
		expect(progress.heard(here, state(null), true), 'plays on unreported').toBeNull();
		progress.report(now);
		expect(starts()).toEqual([true, false]);
	});

	it('takes over on a new connection, unless another device took over meanwhile', () => {
		progress.started();
		progress.report(now);
		progress.heard(here, state(here), true);

		progress.connected();
		expect(progress.heard(here, state(here), true), 'a connection gone still plays').toBe('report');
		progress.report(now);
		expect(starts()).toEqual([true, true]);

		progress.connected();
		expect(progress.heard(here, state('2'), true)).toBe('stop');
		progress.report({ ...now, status: 'paused' });
		expect(sent, 'stopping does not take it back').toHaveLength(2);
	});

	it('takes nothing over on a new connection while not playing', () => {
		progress.started();
		progress.report(now);
		progress.heard(here, state(here), true);

		progress.connected();
		expect(progress.heard(here, state('2'), false)).toBeNull();
		progress.report({ ...now, status: 'paused' });
		expect(progress.heard(here, state(here), false), 'as a tab that is idle').toBeNull();
		expect(starts()).toEqual([true]);
	});

	it('takes over when nothing is named, as after the server restarts', () => {
		progress.started();
		progress.report(now);
		progress.heard(here, state(here), true);
		expect(progress.heard(here, state(null), true)).toBe('report');
		progress.report(now);
		expect(starts()).toEqual([true, true]);
		expect(progress.heard(here, state(null), false), 'not playing').toBeNull();
	});

	it('takes nothing over once the user pauses a start it could not yet send', () => {
		online = false;
		progress.started();
		progress.report(now);
		progress.paused();
		online = true;

		progress.connected();
		expect(progress.heard(here, state('2'), false)).toBeNull();
		progress.report({ ...now, status: 'paused' });
		expect(sent).toEqual([]);
	});

	it('takes nothing over with a stop, which the server refuses as one', () => {
		progress.started();
		progress.report({ ...now, status: 'stopped' });
		progress.report(now);
		expect(starts()).toEqual([false, true]);
	});

	it('keeps playback the user started here while unable to say so', () => {
		online = false;
		progress.started();
		progress.report(now);
		online = true;

		progress.connected();
		expect(progress.heard(here, state('2'), true)).toBe('report');
		progress.report(now);
		expect(starts()).toEqual([true]);
	});
});
