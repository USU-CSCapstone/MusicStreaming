import { describe, expect, it } from 'vitest';
import type { PlayReport } from './api/types';
import { PlayRecorder } from './plays';

function recorder() {
	const sent: PlayReport[] = [];
	const closing: boolean[] = [];
	const r = new PlayRecorder(async (plays, keepalive) => {
		sent.push(...plays);
		closing.push(keepalive);
	});
	return { r, sent, closing };
}

const album = { type: 'album', id: '101' } as const;

describe('PlayRecorder', () => {
	it('reports a play when it starts and when it ends, under one ID', () => {
		const { r, sent } = recorder();
		r.start('1', '7', album);
		r.end('finished');
		expect(sent.map((p) => [p.trackId, p.end])).toEqual([
			['7', null],
			['7', 'finished']
		]);
		expect(sent[0].playId).toBe(sent[1].playId);
		expect(sent[0].context).toEqual(album);
	});

	it('counts audio heard, not jumps or time paused', () => {
		const { r, sent } = recorder();
		r.start('1', '7', album);
		for (let t = 0.25; t <= 10; t += 0.25) r.heard(t);
		r.seeked(100);
		r.heard(100.25);
		r.heard(150); // A jump the player did not announce.
		r.seeked(5);
		r.heard(5.5); // Replaying counts again.
		r.paused();
		expect(sent.at(-1)!.listenTimeMs).toBe(10_000 + 250 + 500);
	});

	it('skips the play under way when another starts, and starts the next from nothing', () => {
		const { r, sent } = recorder();
		r.start('1', '7', album);
		r.heard(0.5);
		r.start('1', '8', album);
		expect(sent.map((p) => [p.trackId, p.end, p.listenTimeMs])).toEqual([
			['7', null, 0],
			['7', 'skipped', 500],
			['8', null, 0]
		]);
		expect(sent[2].playId).not.toBe(sent[0].playId);
	});

	it('sends the last report to outlive the page, and nothing once it has ended', () => {
		const { r, sent, closing } = recorder();
		r.start('1', '7', album);
		r.end('stopped', true);
		r.end('stopped', true);
		r.paused();
		expect(sent).toHaveLength(2);
		expect(closing).toEqual([false, true]);
	});
});
