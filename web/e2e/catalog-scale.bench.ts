import { rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { test } from '@playwright/test';
import { serve } from './catalog-scale/serve.mjs';

// The device-catalog scale gate, on the library the
// server crate's `write_device_scale_files` test writes. Run it with
// `pnpm wasm && pnpm exec playwright test catalog-scale` (the run serves the wasm build as it is).
//
// CDP's `Emulation.setCPUThrottlingRate` reaches pages only ("Operation is only supported for
// pages, not workers"), and the catalog runs in a worker. So the phone stand-in is the whole
// browser held to a quarter of one CPU in 4 ms slices by a cgroup quota (Linux with systemd), which
// slows sustained work 4x or more, as the table's CPU calibration rows show: the quota also takes
// the browser's background threads (wasm tier-up compiles), so it is a pessimistic bound.

const PORT = 4180;
const throttler = fileURLToPath(new URL('./catalog-scale/throttled-chromium.sh', import.meta.url));

test.describe.configure({ mode: 'serial' });
test.setTimeout(0);

let server: Awaited<ReturnType<typeof serve>>;
test.beforeAll(async () => {
	server = await serve(PORT);
});
test.afterAll(() => server.close());

const runs = [
	{ name: 'chromium', label: 'chromium, unthrottled', throttled: false },
	{
		name: 'chromium',
		label: 'chromium, a quarter of one CPU (4x slower or worse)',
		throttled: true
	},
	{ name: 'webkit', label: 'webkit, unthrottled', throttled: false }
] as const;

for (const { name, label, throttled } of runs) {
	test(label, async ({ playwright }) => {
		test.skip(
			throttled && process.platform !== 'linux',
			'the CPU stand-in needs Linux and systemd'
		);
		const type = playwright[name];
		// The full Chromium, not the headless shell, which lacks `measureUserAgentSpecificMemory`.
		const chromium = name === 'chromium' ? playwright.chromium.executablePath() : undefined;
		// A persistent profile, since a private one has a small storage quota.
		const profile = fileURLToPath(
			new URL(`../../target/catalog-scale-profile-${name}`, import.meta.url)
		);
		await rm(profile, { recursive: true, force: true });
		let context;
		try {
			context = await type.launchPersistentContext(
				profile,
				throttled
					? {
							executablePath: throttler,
							env: { ...process.env, CHROMIUM: chromium! } as Record<string, string>
						}
					: { executablePath: chromium }
			);
		} catch (error) {
			test.skip(name === 'webkit', `${name} cannot launch here: ${error}`);
			throw error;
		}
		try {
			const page = await context.newPage();
			page.on('pageerror', (error) => console.log(`page error: ${error}`));
			page.on(
				'console',
				(message) => message.text().startsWith('progress') && console.log(message.text())
			);
			await page.goto(`http://localhost:${PORT}/`);
			// eslint-disable-next-line @typescript-eslint/no-explicit-any
			const results = await page.evaluate(() => (window as any).runBench());
			console.log(`\n=== ${label} ===\n${results}\n`);
		} finally {
			await context.close();
			await rm(profile, { recursive: true, force: true });
		}
	});
}
