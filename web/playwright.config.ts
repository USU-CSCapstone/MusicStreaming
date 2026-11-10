import { defineConfig } from '@playwright/test';

export default defineConfig({
	// Builds the web app and the server, and makes the music, before any server starts.
	globalSetup: './e2e/global-setup.ts',
	testMatch: '**/*.e2e.{ts,js}',
	// One server per worker, each a Rust process of its own.
	workers: 3
});
