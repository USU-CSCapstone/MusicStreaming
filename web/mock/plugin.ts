// Serves the mock API at /api/v1 from `vite dev` and `vite preview`, until the
// Rust server implements it. Reads the database the real scanner writes under
// $JEWELCASE_DATA_DIR, the server's own setting, resolved like any path from the shell
// (default: the repository's `data/`).

import { resolve } from 'node:path';
import type { Plugin } from 'vite';
import { createApi } from './api.ts';

export function mockApi(): Plugin {
	const data = process.env.JEWELCASE_DATA_DIR
		? resolve(process.env.JEWELCASE_DATA_DIR)
		: resolve(import.meta.dirname, '../../data');
	const dbPath = resolve(data, 'state/jewelcase.db');
	// Installed plugins sit beside the scanner's state, never in the music library.
	const api = createApi(dbPath, resolve(data, 'mock-plugins'));
	return {
		name: 'jewelcase-mock-api',
		configureServer(server) {
			server.middlewares.use('/api/v1', api);
			server.config.logger.info(`  mock API reading ${dbPath}`);
		},
		configurePreviewServer(server) {
			server.middlewares.use('/api/v1', api);
		}
	};
}
