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
/** The value at the `p`th percentile of `a`. */
const percentile = (a, p) =>
	[...a].sort((x, y) => x - y)[Math.min(a.length - 1, Math.floor((a.length * p) / 100))];

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

/** The lines of the body of `response` as they arrive. */
async function* lines(response) {
	const decoder = new TextDecoder();
	let rest = '';
	for await (const chunk of response.body) {
		rest += decoder.decode(chunk, { stream: true });
		const parts = rest.split('\n');
		rest = parts.pop();
		yield* parts;
	}
	if (rest) yield rest;
}

/** Bytes of storage the origin uses, as the browser estimates. */
async function storageUsage() {
	return (await navigator.storage.estimate()).usage;
}

/** Streams `/data/device.db` onto OPFS as `import.db`, then checks and opens it. */
async function importPhase(rows) {
	const usage = await storageUsage();
	const start = now();
	const response = await fetch('/data/device.db');
	const size = Number(response.headers.get('content-length'));
	const target = await Import.begin('import.db', size);
	let longest = 0;
	for await (const chunk of response.body) {
		const before = now();
		target.write(chunk);
		longest = Math.max(longest, now() - before);
	}
	target.finish();
	const streamed = now() - start;
	const catalog = await Catalog.open('import.db');
	const check = now();
	const verdict = catalog.quickCheck();
	const checked = now() - check;
	catalog.free();
	rows.push(
		['bootstrap by file import', 'database file', mb(size)],
		['bootstrap by file import', 'stream onto OPFS (finish included)', ms(streamed)],
		['bootstrap by file import', 'longest single write call', ms(longest)],
		['bootstrap by file import', 'quick_check', `${ms(checked)} (${verdict})`],
		['bootstrap by file import', 'stream, open and check, total', ms(now() - start)],
		['bootstrap by file import', 'OPFS usage grew by', mb((await storageUsage()) - usage)]
	);
}

/** Applies `/data/json/*.jsonl` into a new `json.db` in chunks of `chunk` rows, a transaction each. */
async function jsonPhase(rows, chunk) {
	const index = await (await fetch('/data/index.json')).json();
	const usage = await storageUsage();
	const catalog = await Catalog.open('json.db');
	const times = [];
	let total = 0;
	let bytes = 0;
	const start = now();
	for (const { table, size } of index.tables) {
		bytes += size;
		let batch = [];
		const flush = () => {
			const at = now();
			try {
				catalog.applyRows(table, `[${batch.join(',')}]`);
			} catch (error) {
				throw new Error(`${table} after ${total} rows: ${error}`, { cause: error });
			}
			if (times.length % 20 === 0)
				console.log(`progress ${table} ${total} rows, ${ms(now() - start)}`);
			times.push(now() - at);
			total += batch.length;
			batch = [];
		};
		for await (const line of lines(await fetch(`/data/json/${table}.jsonl`))) {
			batch.push(line);
			if (batch.length === chunk) flush();
		}
		if (batch.length) flush();
	}
	const elapsed = now() - start;
	const section = `bootstrap by JSON apply (${chunk} rows a transaction)`;
	rows.push(
		[section, 'JSON', mb(bytes)],
		[section, 'rows applied', String(total)],
		[section, 'fetch and apply, total', ms(elapsed)],
		[
			section,
			'transaction (blocks queries)',
			`median ${ms(median(times))}, p95 ${ms(percentile(times, 95))}, max ${ms(Math.max(...times))}`
		],
		[section, 'OPFS usage grew by', mb((await storageUsage()) - usage)]
	);
	catalog.free();
}

const TRACK = `json_object('id', id, 'title', title, 'albumId', album_id, 'durationUs', duration_us)`;
const MAX = '9223372036854775807';
const ALBUM_ORDER = `disc_number, ifnull(track_number, ${MAX}), id`;
// Each sort the tracks list offers: its index's columns, and how a keyset cursor carries each.
const SORTS = {
	name: [
		['sort_key', 'blob'],
		['id', 'int']
	],
	artist: [
		['artist_sort_key', 'blob'],
		['sort_key', 'blob'],
		['id', 'int']
	],
	album: [
		['album_sort_key', 'blob'],
		['album_id', 'int'],
		['disc_number', 'int'],
		[`ifnull(track_number, ${MAX})`, 'int'],
		['id', 'int']
	],
	added: [
		['added_at', 'int'],
		['id', 'int']
	],
	duration: [
		['duration_us', 'int'],
		['id', 'int']
	],
	release: [
		['release_date', 'text'],
		['album_sort_key', 'blob'],
		['album_id', 'int'],
		['disc_number', 'int'],
		[`ifnull(track_number, ${MAX})`, 'int'],
		['id', 'int']
	]
};

/** Opens `db` cold and measures what a page waits for. */
async function queryPhase(rows, db, spawned) {
	const section = `queries on ${db}`;
	const initStart = now();
	const exports = await init({ module_or_path: '/catalog-wasm/jewelcase_catalog_wasm_bg.wasm' });
	const initialised = now() - initStart;
	const openStart = now();
	const catalog = await Catalog.open(db);
	const opened = now() - openStart;
	const firstStart = now();
	const first = catalog.sql(
		`SELECT json_group_array(${TRACK}) FROM (SELECT * FROM tracks WHERE library_id = 1 ORDER BY sort_key, id LIMIT 100)`,
		[]
	);
	const answered = now() - firstStart;
	rows.push(
		[section, 'wasm instantiate', ms(initialised)],
		[section, 'open on OPFS', ms(opened)],
		[section, 'first page query', ms(answered)],
		[section, 'worker spawn to first answer', ms(Date.now() - spawned)],
		[section, 'first page bytes', String(first.length)],
		[
			section,
			'database size, pages in use',
			mb(
				Number(
					catalog.sql(
						'SELECT CAST(page_count * page_size AS TEXT) FROM pragma_page_count, pragma_page_size',
						[]
					)
				)
			)
		]
	);

	for (const [name, columns] of Object.entries(SORTS)) {
		const order = columns.map(([c]) => c).join(', ');
		const cursor = columns
			.map(([c, kind]) =>
				kind === 'blob' ? `hex(${c})` : kind === 'int' ? `CAST(${c} AS TEXT)` : c
			)
			.join(', ');
		const marks = columns
			.map(([, kind]) =>
				kind === 'blob' ? 'unhex(?)' : kind === 'int' ? 'CAST(? AS INTEGER)' : '?'
			)
			.join(', ');
		const page = (after) =>
			`SELECT json_group_array(${TRACK}) FROM (SELECT * FROM tracks WHERE library_id = 1 ${after ? `AND (${order}) > (${marks})` : ''} ORDER BY ${order} LIMIT 100)`;
		const start = JSON.parse(
			catalog.sql(
				`SELECT json_array(${cursor}) FROM tracks WHERE library_id = 1 ORDER BY ${order} LIMIT 1 OFFSET (SELECT count(*) / 2 FROM tracks)`,
				[]
			)
		);
		rows.push(
			[section, `tracks by ${name}: page 1 of 100`, timed(() => catalog.sql(page(false), []), 20)],
			[
				section,
				`tracks by ${name}: page from the middle (keyset)`,
				timed(() => catalog.sql(page(true), start), 20)
			]
		);
	}

	const tags = JSON.parse(
		catalog.sql(
			`SELECT json_group_array(json_array(CAST(id AS TEXT), name)) FROM tags WHERE library_id = 1 AND (type = 'mood' OR name = (SELECT name FROM tags WHERE type = 'genre' ORDER BY track_count DESC LIMIT 1))`,
			[]
		)
	);
	for (const [id, name] of tags) {
		const total = catalog.sql(
			`SELECT CAST(count(*) AS TEXT) FROM track_tags tt JOIN tracks t ON t.id = tt.track_id WHERE tt.tag_id = CAST(? AS INTEGER) AND t.library_id = 1 AND t.missing_since IS NULL`,
			[id]
		);
		rows.push([
			section,
			`tag-filtered total: ${name} (${total} tracks)`,
			timed(
				() =>
					catalog.sql(
						`SELECT CAST(count(*) AS TEXT) FROM track_tags tt JOIN tracks t ON t.id = tt.track_id WHERE tt.tag_id = CAST(? AS INTEGER) AND t.library_id = 1 AND t.missing_since IS NULL`,
						[id]
					),
				10
			)
		]);
		rows.push([
			section,
			`  same, counting the tag's rows only: ${name}`,
			timed(
				() =>
					catalog.sql(
						'SELECT CAST(count(*) AS TEXT) FROM track_tags WHERE tag_id = CAST(? AS INTEGER)',
						[id]
					),
				10
			)
		]);
	}

	// The shared catalog SQL's totals (`crates/catalog`): a single tag filtering alone has its
	// stored count, an unfiltered list the library's, and a filter of several groups counts from its
	// smallest group instead of walking the library.
	const [genre, mood] = tags
		.filter(([, name]) => name.startsWith('Genre') || name === 'Mood 100000ppm')
		.sort(([, a], [, b]) => (a < b ? 1 : -1))
		.map(([id]) => id);
	const exists =
		'EXISTS (SELECT 1 FROM track_tags WHERE track_id = t.id AND tag_id = CAST(? AS INTEGER))';
	rows.push(
		[
			section,
			'total, unfiltered list (stored)',
			timed(
				() => catalog.sql('SELECT CAST(track_count AS TEXT) FROM libraries WHERE id = 1', []),
				20
			)
		],
		[
			section,
			'total, one tag alone (stored): genre 0',
			timed(
				() =>
					catalog.sql(
						'SELECT CAST(track_count AS TEXT) FROM tags WHERE library_id = 1 AND id = CAST(? AS INTEGER)',
						[genre]
					),
				20
			)
		],
		[
			section,
			'total, one tag alone (stored): mood 100000ppm',
			timed(
				() =>
					catalog.sql(
						'SELECT CAST(track_count AS TEXT) FROM tags WHERE library_id = 1 AND id = CAST(? AS INTEGER)',
						[mood]
					),
				20
			)
		],
		[
			section,
			'total, genre 0 + mood 100000ppm, counted from the smaller group',
			timed(
				() =>
					catalog.sql(
						`SELECT CAST(count(*) AS TEXT) FROM (SELECT track_id AS id FROM track_tags WHERE tag_id = CAST(? AS INTEGER)) lead CROSS JOIN tracks t ON t.id = lead.id WHERE t.library_id = 1 AND ${exists}`,
						[mood, genre]
					),
				10
			)
		],
		[
			section,
			'  same, walking the library and checking each track (before counting from the smallest group)',
			timed(
				() =>
					catalog.sql(
						`SELECT CAST(count(*) AS TEXT) FROM tracks t WHERE t.library_id = 1 AND ${exists} AND ${exists}`,
						[genre, mood]
					),
				3
			)
		]
	);

	const albumTracks = `SELECT json_group_array(${TRACK}) FROM (SELECT * FROM tracks WHERE album_id = CAST(? AS INTEGER) ORDER BY ${ALBUM_ORDER})`;
	const albums = JSON.parse(
		catalog.sql(
			`SELECT json_group_array(CAST(id AS TEXT)) FROM (SELECT id FROM albums WHERE library_id = 1 ORDER BY id LIMIT 100 OFFSET (SELECT count(*) / 3 FROM albums))`,
			[]
		)
	);
	const largest = catalog.sql(
		`SELECT CAST(id AS TEXT) FROM albums WHERE library_id = 1 ORDER BY track_count DESC LIMIT 1`,
		[]
	);
	rows.push(
		[section, 'album tracks: 100 albums', timed((i) => catalog.sql(albumTracks, [albums[i]]), 100)],
		[
			section,
			'album tracks: the largest album',
			timed(() => catalog.sql(albumTracks, [largest]), 5)
		]
	);

	// A recipe's read: every track's ID in each sort's order, from the sort's index alone.
	let ids;
	for (const [name, columns] of Object.entries(SORTS)) {
		const order = columns.map(([c]) => c).join(', ');
		const sql = `SELECT id FROM tracks WHERE library_id = 1 ORDER BY ${order}`;
		rows.push([
			section,
			`read every track ID by ${name}`,
			timed(() => (ids = catalog.ids(sql)), 3)
		]);
	}
	const hashStart = now();
	await crypto.subtle.digest('SHA-256', ids);
	rows.push(
		[
			section,
			`SHA-256 of ${ids.length} IDs (WebCrypto, stand-in for the core's hash)`,
			ms(now() - hashStart)
		],
		[section, 'wasm linear memory after the queries', mb(exports.memory.buffer.byteLength)]
	);
	rows.push([section, 'CPU calibration (100M integer steps)', ms(spin().ms)]);
}

/** Runs the phase a message names and posts its table rows, or the error with the rows so far. */
self.onmessage = async ({ data: { phase, spawned, chunk, db } }) => {
	const rows = [];
	try {
		if (phase === 'import' || phase === 'json') {
			await init({ module_or_path: '/catalog-wasm/jewelcase_catalog_wasm_bg.wasm' });
			rows.push([phase, 'CPU calibration (100M integer steps)', ms(spin().ms)]);
			if (phase === 'import') await importPhase(rows);
			else await jsonPhase(rows, chunk);
		} else {
			await queryPhase(rows, db, spawned);
		}
		postMessage({ rows });
	} catch (error) {
		postMessage({ error: `${phase}: ${error?.stack ?? error}`, rows });
	}
};
postMessage({ ready: true });
