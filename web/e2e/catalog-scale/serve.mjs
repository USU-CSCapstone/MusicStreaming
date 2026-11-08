// Serves the device-catalog scale gate: its page and worker, the catalog wasm build, and the
// files the server crate's `write_device_scale_files` test wrote into `target/catalog-scale`.
// Cross-origin isolated, which `measureUserAgentSpecificMemory` needs. The spec starts it.
//
// To run it by hand, on a phone included, from `web/`: `pnpm wasm && node e2e/catalog-scale/serve.mjs`
// (port 4180, or PORT). OPFS needs a secure context, and a phone cannot reach this machine as
// `localhost`, so put https in front of it, which must pass the COOP/COEP headers through:
// `tailscale serve --bg 4180` or `cloudflared tunnel --url http://localhost:4180`. Open the https
// URL in Safari and tap Run. The page downloads the database file, about 330 MB.
import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import { extname, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url)).replace(/\/$/, '');
const data = process.env.SCALE_DATA ?? resolve(here, '../../../target/catalog-scale');
const roots = [
	['/catalog-wasm/', resolve(here, '../../src/lib/catalog-wasm')],
	['/data/', data],
	['/', here]
];
const types = {
	'.html': 'text/html',
	'.js': 'text/javascript',
	'.wasm': 'application/wasm'
};

/** Starts the server on `port`, all interfaces, and resolves once it listens. */
export function serve(port) {
	const server = createServer(async (request, response) => {
		response.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
		response.setHeader('Cross-Origin-Embedder-Policy', 'require-corp');
		try {
			const path = decodeURIComponent(new URL(request.url, 'http://x').pathname);
			const [prefix, root] = roots.find(([prefix]) => path.startsWith(prefix));
			const file = resolve(root, path === '/' ? 'index.html' : path.slice(prefix.length));
			if (!file.startsWith(root + sep)) throw new Error('outside');
			const info = await stat(file);
			if (!info.isFile()) throw new Error('not a file');
			const { size } = info;
			response.writeHead(200, {
				'Content-Type': types[extname(file)] ?? 'application/octet-stream',
				'Content-Length': size
			});
			createReadStream(file).pipe(response);
		} catch {
			response.writeHead(404).end();
		}
	});
	return new Promise((done) => server.listen(port, () => done(server)));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	const port = Number(process.env.PORT ?? 4180);
	await serve(port);
	console.log(`catalog scale gate: http://localhost:${port}/`);
}
