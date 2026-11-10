import { defineConfig } from '@playwright/test';

// Specs listed here run in `real`, against a real server; the rest run in `fake`, against the mock
// API.
const migrated = ['**/tags.e2e.ts', '**/sorting.e2e.ts', '**/scanning.e2e.ts', '**/app.e2e.ts'];

export default defineConfig({
	globalSetup: './e2e/global-setup.ts',
	webServer: {
		command: 'npm run build && npm run preview',
		port: 4173,
		// Tests serve their own fixtures; point the mock at nothing so a local scan cannot leak in.
		env: { JEWELCASE_DATA_DIR: './e2e/no-data' }
	},
	// One server per worker, each a Rust process of its own.
	workers: 3,
	projects: [
		{
			name: 'fake',
			testMatch: '**/*.e2e.{ts,js}',
			testIgnore: migrated,
			use: { baseURL: 'http://localhost:4173' }
		},
		{ name: 'real', testMatch: migrated }
	]
});
