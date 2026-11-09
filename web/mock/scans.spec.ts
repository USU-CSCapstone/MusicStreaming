import { describe, expect, it } from 'vitest';
import { ScanSimulator, scanRoute } from './scans';

/** A simulator over library `1` of 100 files in two folders, whose clock the test moves. */
function simulator() {
	const clock = { now: 0 };
	const sim = new ScanSimulator({
		files: (lib) => (lib === '1' ? 100 : null),
		folders: () => ['A', 'B'],
		now: () => clock.now
	});
	return { sim, clock };
}

describe('ScanSimulator', () => {
	it('runs a scan through its folders to completion', () => {
		const { sim, clock } = simulator();
		const started = sim.start('1')!;
		expect(started).toMatchObject({ trigger: 'manual', state: 'running', currentFolder: 'A' });

		clock.now = 2000;
		expect(sim.list('1')).toMatchObject([
			{ id: started.id, state: 'running', filesProcessed: 67, currentFolder: 'B' }
		]);

		clock.now = 3000;
		expect(sim.list('1')).toMatchObject([
			{ id: started.id, state: 'completed', filesSeen: 100, currentFolder: null }
		]);
	});

	it('queues one scan to follow a running one, and answers it again while it waits', () => {
		const { sim, clock } = simulator();
		const running = sim.start('1')!;
		const next = sim.start('1')!;
		expect(next.state).toBe('queued');
		expect(sim.start('1')!.id, 'pressing again').toBe(next.id);
		expect(sim.list('1')!.map((s) => [s.id, s.state])).toEqual([
			[next.id, 'queued'],
			[running.id, 'running']
		]);

		clock.now = 3500;
		expect(
			sim.list('1')!.map((s) => [s.id, s.state]),
			'the first finished'
		).toEqual([
			[next.id, 'running'],
			[running.id, 'completed']
		]);
		expect(sim.list('1')![0].startedAt).toBe(new Date(3000).toISOString());
	});

	it('keeps only the last finished scan', () => {
		const { sim, clock } = simulator();
		sim.start('1');
		clock.now = 5000;
		const second = sim.start('1')!;
		clock.now = 10_000;
		expect(sim.list('1')!.map((s) => s.id)).toEqual([second.id]);
	});

	it('lists the seeded scan as the last to finish', () => {
		const seeded = { ...simulator().sim.start('1')!, id: '99', finishedAt: '2026-01-01T00:00:00Z' };
		const sim = new ScanSimulator({ files: () => 1, seed: () => seeded });
		expect(sim.list('1')).toEqual([seeded]);
	});
});

describe('scanRoute', () => {
	const PATH = '/admin/libraries/1/scans';

	it('starts and lists scans', () => {
		const { sim } = simulator();
		const started = scanRoute(sim, 'POST', PATH);
		expect(started?.status).toBe(202);
		expect(scanRoute(sim, 'GET', PATH)).toEqual({ status: 200, body: { items: [started!.body] } });
	});

	it('answers 404 for a library that does not exist, and leaves other paths alone', () => {
		const { sim } = simulator();
		expect(scanRoute(sim, 'POST', '/admin/libraries/2/scans')?.status).toBe(404);
		expect(scanRoute(sim, 'GET', '/admin/plugins')).toBeUndefined();
	});
});
