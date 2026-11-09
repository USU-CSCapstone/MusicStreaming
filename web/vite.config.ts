import { defineConfig } from 'vitest/config';
import { playwright } from '@vitest/browser-playwright';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { mockApi } from './mock/plugin.ts';

// Under `docker compose up`, the Rust server answers the API, realtime connection included; run
// on its own, the mock does, without one: the app then shows itself Offline 3 s after it starts,
// though pages still load.
const apiProxy = process.env.API_PROXY;

export default defineConfig({
	server: apiProxy ? { proxy: { '/api': { target: apiProxy, ws: true } } } : {},
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},

			// SPA mode: static files served by the Rust server, unknown paths fall back to index.html
			adapter: adapter({ fallback: 'index.html' }),

			// The offline shell is one page served for every path, so its script URLs must not depend on
			// the path it is asked at (the preview server writes them relative to it by default).
			paths: { relative: false },

			// Registered by `hooks.client.ts`, in production builds only: in development the
			// worker would serve a stale app.
			serviceWorker: { register: false }
		}),
		apiProxy ? [] : mockApi()
	],
	test: {
		expect: { requireAssertions: true },
		projects: [
			{
				extends: './vite.config.ts',
				test: {
					name: 'client',
					browser: {
						enabled: true,
						provider: playwright(),
						instances: [{ browser: 'chromium', headless: true }]
					},
					include: ['src/**/*.svelte.{test,spec}.{js,ts}'],
					exclude: ['src/lib/server/**']
				}
			},

			{
				extends: './vite.config.ts',
				test: {
					name: 'server',
					environment: 'node',
					setupFiles: ['./core-node.ts'],
					include: ['src/**/*.{test,spec}.{js,ts}', 'mock/**/*.{test,spec}.ts'],
					exclude: ['src/**/*.svelte.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
