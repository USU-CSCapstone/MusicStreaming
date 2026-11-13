import { expect, test } from './test';

test.use({ serviceWorkers: 'allow' });

test('Chromium finds the app installable', async ({ page, context, server }) => {
	await page.goto(`${server.url}/setup`);
	await page.evaluate(() => navigator.serviceWorker.ready);
	const cdp = await context.newCDPSession(page);
	const { url, errors } = await cdp.send('Page.getAppManifest');
	expect(errors).toEqual([]);
	expect(url).toMatch(/\/manifest\.webmanifest$/);
	const manifest = await page.evaluate(async (url) => (await fetch(url)).json(), url!);
	expect(manifest).toMatchObject({ display: 'standalone' });
	expect(manifest.icons.map((icon: { purpose: string }) => icon.purpose).sort()).toEqual([
		'any',
		'any',
		'maskable'
	]);
	const { installabilityErrors } = await cdp.send('Page.getInstallabilityErrors');
	expect(installabilityErrors).toEqual([]);
});
