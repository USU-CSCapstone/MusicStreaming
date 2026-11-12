import { defineConfig } from '@playwright/test';

// The scale gate is a benchmark: it runs only when its name is on the command line, and it
// serves its own page, so it neither builds nor starts a server.
const scale = process.argv.some((arg) => arg.includes('catalog-scale'));

export default defineConfig(
	scale
		? { testMatch: '**/*.bench.ts' }
		: {
				// Builds the web app and the server, and makes the music, before any server starts.
				globalSetup: './e2e/global-setup.ts',
				testMatch: '**/*.e2e.{ts,js}',
				// One server per worker, each a Rust process of its own.
				workers: 3
			}
);
