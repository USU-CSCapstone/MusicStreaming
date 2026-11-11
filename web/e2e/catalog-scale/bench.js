// The page of the device-catalog scale gate: runs each phase in a fresh worker and prints the
// table. `runBench()` is what the Playwright spec calls; the Run button is for a phone.

/** Runs one phase in a new worker and resolves with its table rows. */
function phase(message) {
	return new Promise((resolve, reject) => {
		const spawned = Date.now();
		const worker = new Worker('/worker.js', { type: 'module' });
		worker.onerror = (event) => reject(new Error(event.message));
		worker.onmessage = ({ data }) => {
			if (data.ready) return worker.postMessage({ ...message, spawned });
			if (data.error || message.phase !== 'query') {
				worker.terminate();
				if (data.error) reject(new Error(data.error));
				else resolve(data.rows);
				return;
			}
			// Only a window can ask, and it counts its workers, so ask before this one ends.
			memory().then((row) => {
				worker.terminate();
				resolve([...data.rows, [`queries on ${message.db}`, ...row]]);
			});
		};
	});
}

/** The agent's memory in use, and the worker's share of it, where the browser can say. */
async function memory() {
	const measure = 'memory in use (measureUserAgentSpecificMemory)';
	if (!performance.measureUserAgentSpecificMemory) return [measure, 'not in this browser'];
	try {
		const { bytes, breakdown } = await performance.measureUserAgentSpecificMemory();
		/** Formats a byte count in mebibytes. */
		const mb = (n) => `${(n / 1048576).toFixed(1)} MB`;
		const worker = breakdown
			.filter(({ attribution }) => attribution.some(({ url }) => url.endsWith('/worker.js')))
			.reduce((sum, { bytes }) => sum + bytes, 0);
		return [measure, `${mb(bytes)} in all, ${mb(worker)} attributed to the worker`];
	} catch (error) {
		return [measure, `failed: ${error}`];
	}
}

/** All phases' rows, as aligned text. */
function table(rows) {
	const width = Math.max(...rows.map(([, measure]) => measure.length));
	let section;
	return rows
		.map(([name, measure, value]) => {
			const heading = name === section ? '' : `\n${(section = name)}\n`;
			return `${heading}  ${measure.padEnd(width)}  ${value}`;
		})
		.join('\n');
}

/** Runs every phase and resolves with the results table as text. */
window.runBench = async () => {
	const rows = [];
	const env = [
		['environment', 'user agent', navigator.userAgent],
		['environment', 'cross-origin isolated', String(crossOriginIsolated)],
		['environment', 'hardware concurrency', String(navigator.hardwareConcurrency)],
		[
			'environment',
			'storage quota',
			`${((await navigator.storage.estimate()).quota / 1048576).toFixed(0)} MB`
		]
	];
	rows.push(...env);
	for (const message of [
		{ phase: 'import' },
		{ phase: 'index' },
		{ phase: 'recipe' },
		{ phase: 'query', db: 'import.db' }
	]) {
		rows.push(...(await phase(message)));
	}
	return table(rows);
};

/** Runs the bench on a button press and shows the table, or the error, on the page. */
document.querySelector('#start').onclick = async () => {
	const out = document.querySelector('#out');
	out.textContent = 'Running, this takes minutes on a phone...';
	try {
		out.textContent = await window.runBench();
	} catch (error) {
		out.textContent = String(error);
	}
};
