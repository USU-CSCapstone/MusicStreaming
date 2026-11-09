import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { RUN_ENV, buildServer, generateMusic } from './server';

/**
 * Builds the server and generates the music once for the run, in a folder that also holds every
 * server's folders. The returned function removes it.
 */
export default function globalSetup() {
	buildServer();
	const run = mkdtempSync(join(tmpdir(), 'jewelcase-e2e-'));
	process.env[RUN_ENV] = run;
	try {
		const music = join(run, 'music');
		mkdirSync(music);
		generateMusic(music);
	} catch (error) {
		rmSync(run, { recursive: true, force: true });
		throw error;
	}
	return () => rmSync(run, { recursive: true, force: true });
}
