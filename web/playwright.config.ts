import { defineConfig } from '@playwright/test';

export default defineConfig({
	webServer: {
		command: 'npm run build && npm run preview',
		port: 4173,
		// Tests serve their own fixtures; point the mock at nothing so a local scan cannot leak in.
		env: { JEWELCASE_DATA_DIR: './e2e/no-data' }
	},
	testMatch: '**/*.e2e.{ts,js}'
});
