// The worker of the device-catalog scale gate: one phase of the bench per worker, so each starts
// cold. Messages in: {phase, spawned (epoch ms), ...}; out: {rows: [[section, measure, value]]}.
import init, {
	Catalog,
	Import,
	storeNames,
	storeReserve
} from '/catalog-wasm/jewelcase_catalog_wasm.js';

/** The catalog's wasm build. */
const WASM = '/catalog-wasm/jewelcase_catalog_wasm_bg.wasm';
/** Bytes copied from the download into the pool at a time, as the sync does. */
const CHUNK = 1 << 20;
/** The slicing of an apply and of an index build, as the sync does (`sync.ts`). */
const SLICE_MS = 50;
const APPLY_CHUNK = 50;
const APPLY_MAX = 1000;
const INDEX_CHUNK = 2000;
const INDEX_MAX = 1 << 20;
/** The bandwidths a download is modelled at, in Mbit/s. */
const RATES = [10, 50];

/** Formats a duration in milliseconds. */
const ms = (n) => `${n.toFixed(n < 10 ? 2 : 1)} ms`;
/** Formats a byte count in megabytes. */
const mb = (n) => `${(n / 1048576).toFixed(1)} MB`;
/** The high-resolution clock, in milliseconds. */
const now = () => performance.now();
/** The middle value of `a`, the upper one for an even count. */
const median = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];

/**
 * A fixed amount of integer work, so the table shows how much CPU throttling reaches the worker.
 */
function spin() {
	const start = now();
	let x = 1;
	for (let i = 0; i < 100_000_000; i++) x = (x * 1103515245 + 12345) | 0;
	return { ms: now() - start, x };
}

/** The size of the slice after one of `limit` that took `took` ms, as the sync sizes it. */
const slice = (limit, took, max) =>
	Math.max(1, Math.min(max, limit * 2, took > 0 ? Math.floor((limit * SLICE_MS) / took) : max));

/**
 * Calls `step(limit)` until it says it is done, yielding between calls and sizing each as the
 * sync does (no slice below `floor`), and resolves with how long each call held the worker.
 */
async function sliced(step, first, max, floor = 1) {
	const holds = [];
	let limit = first;
	for (let done = false; !done;) {
		const before = now();
		done = step(limit);
		const took = now() - before;
		holds.push(took);
		limit = Math.max(floor, slice(limit, took, max));
		await new Promise((resolve) => setTimeout(resolve));
	}
	return holds;
}

/** The slices' count and how long they held the worker: median, 99th percentile and longest. */
function holdsRow(holds) {
	const sorted = [...holds].sort((a, b) => a - b);
	const p99 = sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.99))];
	return `${holds.length} slices, median ${ms(median(holds))}, p99 ${ms(p99)}, longest ${ms(sorted[sorted.length - 1])}`;
}

/** Runs `fn` `times` times and reports the first (cold) run, the median and the slowest. */
function timed(fn, times) {
	const all = [];
	for (let i = 0; i < times; i++) {
		const start = now();
		fn(i);
		all.push(now() - start);
	}
	return `first ${ms(all[0])}, median ${ms(median(all))}, max ${ms(Math.max(...all))}`;
}

/** Bytes of storage the origin uses, as the browser estimates. */
async function storageUsage() {
	return (await navigator.storage.estimate()).usage;
}

/**
 * Bootstraps `import.db` as the sync does: downloads `/data/device.db.gz` into a plain OPFS file,
 * then inflates it into the pool from there, checking its hash, and opens it. The download's
 * network time is modelled from its size at each of `RATES`, since the bench serves it locally.
 */
async function importPhase(rows) {
	const section = 'bootstrap by snapshot';
	const usage = await storageUsage();
	const etag = (await (await fetch('/data/sha256')).text()).trim();
	const root = await navigator.storage.getDirectory();
	const handle = await root.getFileHandle('download.gz', { create: true });
	const download = await handle.createSyncAccessHandle();
	const start = now();
	const response = await fetch('/data/device.db.gz');
	let at = 0;
	for await (const chunk of response.body) at += download.write(chunk, { at });
	download.flush();
	const downloaded = now() - start;
	const peak = (await storageUsage()) - usage;
	const inflated = Number(
		(await fetch('/data/device.db', { method: 'HEAD' })).headers.get('content-length')
	);
	const importStart = now();
	const size = download.getSize();
	const target = await Import.begin('import.db');
	let longest = 0;
	const chunk = new Uint8Array(CHUNK);
	for (let from = 0; from < size;) {
		const read = download.read(chunk, { at: from });
		const before = now();
		target.write(chunk.subarray(0, read));
		longest = Math.max(longest, now() - before);
		from += read;
	}
	target.finish(etag);
	const imported = now() - importStart;
	download.close();
	const openStart = now();
	(await Catalog.open('import.db')).free();
	const opened = now() - openStart;
	const peakWithImport = (await storageUsage()) - usage;
	await root.removeEntry('download.gz');
	const device = downloaded + imported + opened;
	rows.push(
		[section, 'database file', mb(inflated)],
		[section, 'gzipped download', mb(size)],
		[section, 'write the download to OPFS (local fetch)', ms(downloaded)],
		[section, 'inflate, hash and import into the pool', ms(imported)],
		[section, 'longest single write call', ms(longest)],
		[section, 'open', ms(opened)],
		[section, 'device time, total', ms(device)],
		...RATES.map((rate) => [
			section,
			`end to end at ${rate} Mbit/s (download modelled)`,
			ms((size * 8) / (rate * 1000) + device)
		]),
		[section, 'OPFS grew by, download alone', mb(peak)],
		[section, 'OPFS grew by, download and import', mb(peakWithImport)],
		[section, 'OPFS grew by, after the download is deleted', mb((await storageUsage()) - usage)]
	);
}

/** Each sort the tracks list and a library recipe offer. */
const SORTS = ['name', 'artist', 'album', 'dateAdded', 'releaseDate', 'duration'];

/** Opens `db` cold and measures what a page waits for, through the catalog's shared queries. */
async function queryPhase(rows, db, spawned, library) {
	const section = `queries on ${db}`;
	const compileStart = now();
	const module = await WebAssembly.compileStreaming(fetch(WASM));
	const compiled = now() - compileStart;
	const initStart = now();
	const exports = await init({ module_or_path: module });
	const initialised = now() - initStart;
	const installStart = now();
	await storeNames();
	const installed = now() - installStart;
	const reserveStart = now();
	await storeReserve(2 * (1 + 2));
	const reserved = now() - reserveStart;
	const openStart = now();
	const catalog = await Catalog.open(db);
	const opened = now() - openStart;
	const query = (name, params) => catalog.query('1', library, name, JSON.stringify(params));
	const firstStart = now();
	const first = query('listTracks', { limit: 100 });
	const answered = now() - firstStart;
	rows.push(
		[section, 'wasm fetch and compile', ms(compiled)],
		[section, 'wasm instantiate', ms(initialised)],
		[section, 'pool install (sahpool)', ms(installed)],
		[section, 'pool reserve for one library', ms(reserved)],
		[section, 'open on OPFS', ms(opened)],
		[section, 'first page query', ms(answered)],
		[section, 'worker spawn to first answer', ms(Date.now() - spawned)],
		[section, 'first page bytes', String(first.length)]
	);

	for (const sort of SORTS) {
		const page = (cursor) => query('listTracks', { sort, limit: 100, cursor });
		const { nextCursor } = JSON.parse(page());
		rows.push(
			[section, `tracks by ${sort}: page 1 of 100`, timed(() => page(), 20)],
			[section, `tracks by ${sort}: page 2 (keyset)`, timed(() => page(nextCursor), 20)]
		);
	}

	// Totals: a single tag filtering alone and the unfiltered list have stored counts; a filter of
	// several groups counts from its smallest group.
	const tags = (params) => JSON.parse(query('listTags', { limit: 100, ...params })).items;
	const moods = tags({ type: 'mood' });
	const [genre] = tags({ type: 'genre', sort: 'trackCount', order: 'desc' });
	const mood = moods.find(({ name }) => name === 'Mood 100000ppm');
	const counted = (tagAll) => query('listTracks', { tagAll, limit: 1 });
	rows.push([section, 'total, unfiltered list (stored)', timed(() => counted(undefined), 20)]);
	for (const { id, name } of [genre, ...moods]) {
		const { total } = JSON.parse(counted(id));
		rows.push([
			section,
			`total, one tag alone: ${name} (${total} tracks)`,
			timed(() => counted(id), 10)
		]);
	}
	rows.push([
		section,
		`total, ${genre.name} + ${mood.name}, counted from the smaller group`,
		timed(() => counted(`${genre.id},${mood.id}`), 10)
	]);

	const albumTracks = (albumId) => query('listTracks', { albumId, sort: 'album', limit: 1000 });
	const albums = JSON.parse(query('listAlbums', { limit: 100 })).items;
	rows.push([
		section,
		'album tracks: 100 albums',
		timed((i) => albumTracks(albums[i].id), albums.length)
	]);

	// A recipe's read: every track's ID in the sort's order, with its list hash.
	for (const sort of SORTS) {
		const recipe = { type: 'library', filters: {}, sort, order: 'asc' };
		rows.push([
			section,
			`resolve and hash a library recipe by ${sort}`,
			timed(() => query('resolveRecipe', { recipe }), 3)
		]);
	}
	rows.push([
		section,
		'wasm linear memory after the queries',
		mb(exports.memory.buffer.byteLength)
	]);
	rows.push([section, 'CPU calibration (100M integer steps)', ms(spin().ms)]);
}

/** Builds the search index of `import.db` and measures it, with the searches it answers. */
async function indexPhase(rows, library) {
	const section = 'search index on import.db';
	const exports = await init({ module_or_path: WASM });
	const catalog = await Catalog.open('import.db');
	const memory = () => mb(exports.memory.buffer.byteLength);
	/** Runs a search for `q` and returns the length of the answer. */
	const search = (q) =>
		catalog.query('1', library, 'search', JSON.stringify({ q, sectionLimit: 12 })).length;
	/** One build in slices, as the sync runs it: its total, with the yields, and its holds. */
	const build = async () => {
		const start = now();
		catalog.indexBegin(library);
		const holds = await sliced((limit) => catalog.indexStep(limit), INDEX_CHUNK, INDEX_MAX);
		return [ms(now() - start), holdsRow(holds)];
	};
	rows.push([section, 'wasm linear memory before the build (SQLite open)', memory()]);
	const [first, firstHolds] = await build();
	rows.push(
		[section, 'first build in slices (a search waits this long)', first],
		[section, 'first build, holds of the worker', firstHolds],
		[section, 'wasm linear memory after the first build', memory()]
	);
	for (let i = 1; i <= 3; i++) {
		const [again, holds] = await build();
		rows.push(
			[section, `rebuild ${i} in slices (a stale index, after its spacing)`, again],
			[section, `rebuild ${i}, holds of the worker`, holds]
		);
	}
	rows.push(
		[section, 'wasm linear memory after three rebuilds', memory()],
		[section, 'search "love" with the index built', timed(() => search('love'), 20)],
		[section, 'search "the beat" with the index built', timed(() => search('the beat'), 20)],
		[section, 'CPU calibration (100M integer steps)', ms(spin().ms)]
	);
	return catalog;
}

/**
 * Resolves a library recipe as a queue start does on the device: the catalog's `resolveRecipe`
 * (read, sort and hash in wasm), then the page-side steps the queue worker adds to its answer.
 */
async function recipePhase(rows, library) {
	const section = 'recipe start on import.db';
	await init({ module_or_path: WASM });
	const catalog = await Catalog.open('import.db');
	for (const sort of ['name', 'album']) {
		const params = JSON.stringify({ recipe: { type: 'library', filters: {}, sort, order: 'asc' } });
		let answer;
		let parsed;
		const resolve = () => (answer = catalog.query('1', library, 'resolveRecipe', params));
		rows.push(
			[section, `resolve and hash by ${sort} (wasm query, JSON out)`, timed(resolve, 3)],
			[section, `  answer size`, mb(answer.length)],
			[
				section,
				`  structuredClone of the answer (the channel hop)`,
				timed(() => structuredClone(answer), 3)
			],
			[section, `  JSON.parse of the answer`, timed(() => (parsed = JSON.parse(answer)), 3)],
			[
				section,
				`  JSON.stringify of the list body`,
				timed(() => JSON.stringify({ libraryId: library, tracks: parsed.ids }), 3)
			],
			[
				section,
				`whole path by ${sort}, as a start runs it`,
				timed(() => {
					const found = JSON.parse(structuredClone(resolve()));
					JSON.stringify({ libraryId: library, tracks: found.ids });
				}, 3)
			]
		);
	}
	rows.push([section, 'CPU calibration (100M integer steps)', ms(spin().ms)]);
	catalog.free();
}

/** The share of the library each catch-up applies, in percent. */
const SHARES = [1, 5, 10, 25];

/**
 * Catches `import.db` up by feed pages holding each of `SHARES` of the library's entities, as
 * the sync does: each page fetched gzipped, parsed and staged, then the staged changes applied
 * in slices, yielding between them. The pages' network time is modelled from
 * their gzipped size at each of `RATES`. The changes rewrite entities the file already holds.
 */
async function catchUpPhase(rows, library) {
	const section = 'catch-up by feed pages';
	await init({ module_or_path: WASM });
	const catalog = await Catalog.open('import.db');
	const total = (name) => JSON.parse(catalog.query('1', library, name, '{"limit":1}')).total;
	const entities = ['listTracks', 'listAlbums', 'listArtists'].map(total).reduce((a, b) => a + b);
	rows.push([section, 'entities', String(entities)]);
	for (const share of SHARES) {
		const wanted = Math.ceil((entities * share) / 100);
		let staged = 0;
		let bytes = 0;
		let through = 0;
		const pullStart = now();
		for (let page = 0; staged < wanted; page++) {
			const response = await fetch(`/data/changes/${String(page).padStart(4, '0')}.json.gz`);
			const body = await response.arrayBuffer();
			bytes += body.byteLength;
			const text = await new Response(
				new Blob([body]).stream().pipeThrough(new DecompressionStream('gzip'))
			).text();
			const { changes, epoch } = JSON.parse(text);
			const taken = changes.slice(0, wanted - staged);
			catalog.stage('library', epoch, JSON.stringify(taken));
			staged += taken.length;
			through = taken[taken.length - 1].position;
		}
		const pulled = now() - pullStart;
		const applyStart = now();
		const holds = await sliced(
			(limit) => catalog.commit('library', BigInt(through), limit),
			APPLY_CHUNK,
			APPLY_MAX,
			APPLY_CHUNK
		);
		const applied = now() - applyStart;
		const label = `${share}% (${staged} changes)`;
		rows.push(
			[section, `${label}: gzipped pages`, mb(bytes)],
			[section, `${label}: fetch, inflate, parse, stage`, ms(pulled)],
			[section, `${label}: apply`, ms(applied)],
			[section, `${label}: holds of the worker`, holdsRow(holds)],
			...RATES.map((rate) => [
				section,
				`${label}: end to end at ${rate} Mbit/s (download modelled)`,
				ms((bytes * 8) / (rate * 1000) + pulled + applied)
			])
		);
	}
	rows.push([section, 'CPU calibration (100M integer steps)', ms(spin().ms)]);
	catalog.free();
}

/** Runs the phase a message names and posts its table rows, or the error with the rows so far. */
self.onmessage = async ({ data: { phase, spawned, db, library } }) => {
	const rows = [];
	try {
		if (phase === 'import') {
			await init({ module_or_path: WASM });
			rows.push([phase, 'CPU calibration (100M integer steps)', ms(spin().ms)]);
			await importPhase(rows);
		} else if (phase === 'index') {
			// Kept open until the page has measured the worker's memory with the index built.
			self.catalog = await indexPhase(rows, library);
		} else if (phase === 'recipe') {
			await recipePhase(rows, library);
		} else if (phase === 'catchUp') {
			await catchUpPhase(rows, library);
		} else {
			await queryPhase(rows, db, spawned, library);
		}
		postMessage({ rows });
	} catch (error) {
		postMessage({ error: `${phase}: ${error?.stack ?? error}`, rows });
	}
};
postMessage({ ready: true });
