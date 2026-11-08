import { defineConfig } from '@playwright/test';

// The scale gate is a benchmark: it runs only when its name is on the command line, and it
// serves its own page, so it neither builds nor starts the app.
const scale = process.argv.some((arg) => arg.includes('catalog-scale'));

export default defineConfig({
	// Tests stub the API in the page, which a service worker's own requests would bypass; the
	// offline and installability specs turn it on for themselves.
	use: { serviceWorkers: 'block' },
	webServer: scale
		? undefined
		: {
				command: 'npm run build && npm run preview',
				port: 4173,
				// Tests serve their own fixtures; point the mock at nothing so a local scan cannot leak in.
				env: { JEWELCASE_DATA_DIR: './e2e/no-data' }
			},
	testMatch: scale ? '**/*.bench.ts' : '**/*.e2e.{ts,js}'
});
