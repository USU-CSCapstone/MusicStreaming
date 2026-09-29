// Runs an installed plugin by starting the prototype's `plugin-run`
// (spikes/plugins/runner), which applies the permissions approved on the Plugins page.

import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import type { PluginRunResult } from '../src/lib/api/plugins.ts';
import { PluginError } from './plugins.ts';

const BIN = resolve(import.meta.dirname, '../../spikes/plugins/target/release/plugin-run');
const TIMEOUT_MS = 330_000; // A little past the runner's own five-minute limit.
const SERVER = process.env.JEWELCASE_SERVER_URL ?? 'http://localhost:8080';

/** Whether the Rust server, and so its scanner's folder watcher, is up. */
async function scannerRunning(): Promise<boolean> {
	try {
		return (await fetch(`${SERVER}/health`, { signal: AbortSignal.timeout(500) })).ok;
	} catch {
		return false;
	}
}

export function pluginRunner(dataDir: string) {
	return async (id: string): Promise<PluginRunResult> => {
		if (!existsSync(BIN)) {
			throw new PluginError(
				503,
				'runner_unavailable',
				'The plugin runner is not built. Run spikes/plugins/build.sh.'
			);
		}
		const log: string[] = [];
		let done: { ok: boolean; summary: string; saved: number } | null = null;
		await new Promise<void>((settle) => {
			const child = spawn(BIN, [id, '--data', dataDir], { stdio: ['ignore', 'pipe', 'pipe'] });
			const timer = setTimeout(() => child.kill('SIGKILL'), TIMEOUT_MS);
			let buffered = '';
			child.stdout.setEncoding('utf8').on('data', (chunk: string) => {
				buffered += chunk;
				const lines = buffered.split('\n');
				buffered = lines.pop() ?? '';
				for (const line of lines) {
					try {
						const msg = JSON.parse(line);
						if (typeof msg.log === 'string') log.push(msg.log);
						if (msg.done) done = msg.done;
					} catch {
						// Not a runner message; ignore it.
					}
				}
			});
			child.stderr.resume();
			child.on('error', () => settle());
			child.on('close', () => {
				clearTimeout(timer);
				settle();
			});
		});
		const result = done ?? {
			ok: false,
			summary: 'The plugin runner stopped without finishing.',
			saved: 0
		};
		return { ...result, log, scannerRunning: await scannerRunning() };
	};
}
