// The worker of the device-catalog scale gate: one phase of the bench per worker, so each starts
// cold. Messages in: {phase, spawned (epoch ms), ...}; out: {rows: [[section, measure, value]]}.
import init, { Catalog, Import } from '/catalog-wasm/jewelcase_catalog_wasm.js';

/** Formats a duration in milliseconds. */
const ms = (n) => `${n.toFixed(n < 10 ? 2 : 1)} ms`;
/** Formats a byte count in megabytes. */
const mb = (n) => `${(n / 1048576).toFixed(1)} MB`;
/** The high-resolution clock, in milliseconds. */
const now = () => performance.now();
/** The middle value of `a`, the upper one for an even count. */
const median = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];

/** A fixed amount of integer work, so the table shows how much CPU throttling reaches the worker. */
function spin() {
	const start = now();
	let x = 1;
	for (let i = 0; i < 100_000_000; i++) x = (x * 1103515245 + 12345) | 0;
	return { ms: now() - start, x };
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

/** Streams `/data/device.db.gz` onto OPFS as `import.db`, inflating it, then opens it. */
async function importPhase(rows) {
	const usage = await storageUsage();
	const start = now();
	// The inflated size, which the import is begun with.
	const head = await fetch('/data/device.db', { method: 'HEAD' });
	const size = Number(head.headers.get('content-length'));
	const response = await fetch('/data/device.db.gz');
	const target = await Import.begin('import.db', size);
	let longest = 0;
	for await (const chunk of response.body) {
		const before = now();
		target.write(chunk);
		longest = Math.max(longest, now() - before);
	}
	target.finish();
	const streamed = now() - start;
	(await Catalog.open('import.db')).free();
	rows.push(
		['bootstrap by file import', 'database file', mb(size)],
		[
			'bootstrap by file import',
			'gzipped download',
			mb(Number(response.headers.get('content-length')))
		],
		['bootstrap by file import', 'stream onto OPFS (finish included)', ms(streamed)],
		['bootstrap by file import', 'longest single write call', ms(longest)],
		['bootstrap by file import', 'stream and open, total', ms(now() - start)],
		['bootstrap by file import', 'OPFS usage grew by', mb((await storageUsage()) - usage)]
	);
}

/** Each sort the tracks list and a library recipe offer. */
const SORTS = ['name', 'artist', 'album', 'dateAdded', 'releaseDate', 'duration'];

/** Opens `db` cold and measures what a page waits for, through the catalog's shared queries. */
async function queryPhase(rows, db, spawned) {
	const section = `queries on ${db}`;
	const initStart = now();
	const exports = await init({ module_or_path: '/catalog-wasm/jewelcase_catalog_wasm_bg.wasm' });
	const initialised = now() - initStart;
	const openStart = now();
	const catalog = await Catalog.open(db);
	const opened = now() - openStart;
	const query = (name, params) => catalog.query('1', '1', name, JSON.stringify(params));
	const firstStart = now();
	const first = query('listTracks', { limit: 100 });
	const answered = now() - firstStart;
	rows.push(
		[section, 'wasm instantiate', ms(initialised)],
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
async function indexPhase(rows) {
	const section = 'search index on import.db';
	const exports = await init({ module_or_path: '/catalog-wasm/jewelcase_catalog_wasm_bg.wasm' });
	const catalog = await Catalog.open('import.db');
	const search = (q) =>
		catalog.query('1', '1', 'search', JSON.stringify({ q, sectionLimit: 12 })).length;
	rows.push(
		[section, 'first build (a search waits this long)', timed(() => catalog.buildIndex('1'), 1)],
		[
			section,
			'rebuild (a stale index, after its spacing)',
			timed(() => catalog.buildIndex('1'), 3)
		],
		[section, 'search "love" with the index built', timed(() => search('love'), 20)],
		[section, 'search "the beat" with the index built', timed(() => search('the beat'), 20)],
		[section, 'wasm linear memory after the build', mb(exports.memory.buffer.byteLength)],
		[section, 'CPU calibration (100M integer steps)', ms(spin().ms)]
	);
	catalog.free();
}

/**
 * Resolves a library recipe as a queue start does on the device: the catalog's `resolveRecipe`
 * (read, sort and hash in wasm), then the page-side steps the queue worker adds to its answer.
 */
async function recipePhase(rows) {
	const section = 'recipe start on import.db';
	await init({ module_or_path: '/catalog-wasm/jewelcase_catalog_wasm_bg.wasm' });
	const catalog = await Catalog.open('import.db');
	for (const sort of ['name', 'album']) {
		const params = JSON.stringify({ recipe: { type: 'library', filters: {}, sort, order: 'asc' } });
		let answer;
		let parsed;
		const resolve = () => (answer = catalog.query('1', '1', 'resolveRecipe', params));
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
				timed(() => JSON.stringify({ libraryId: '1', tracks: parsed.ids }), 3)
			],
			[
				section,
				`whole path by ${sort}, as a start runs it`,
				timed(() => {
					const found = JSON.parse(structuredClone(resolve()));
					JSON.stringify({ libraryId: '1', tracks: found.ids });
				}, 3)
			]
		);
	}
	rows.push([section, 'CPU calibration (100M integer steps)', ms(spin().ms)]);
	catalog.free();
}

/** Runs the phase a message names and posts its table rows, or the error with the rows so far. */
self.onmessage = async ({ data: { phase, spawned, db } }) => {
	const rows = [];
	try {
		if (phase === 'import') {
			await init({ module_or_path: '/catalog-wasm/jewelcase_catalog_wasm_bg.wasm' });
			rows.push([phase, 'CPU calibration (100M integer steps)', ms(spin().ms)]);
			await importPhase(rows);
		} else if (phase === 'index') {
			await indexPhase(rows);
		} else if (phase === 'recipe') {
			await recipePhase(rows);
		} else {
			await queryPhase(rows, db, spawned);
		}
		postMessage({ rows });
	} catch (error) {
		postMessage({ error: `${phase}: ${error?.stack ?? error}`, rows });
	}
};
postMessage({ ready: true });
