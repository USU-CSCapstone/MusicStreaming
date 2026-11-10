// Builds, feeds and runs the real server for the E2E tests.

import { type ChildProcess, execFileSync, spawn } from 'node:child_process';
import {
	cpSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	renameSync,
	rmSync,
	writeFileSync
} from 'node:fs';
import { createServer } from 'node:net';
import { dirname, join, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '../..');
// Where cargo puts the build, as `scripts/wasm.sh` finds it.
const binary = join(
	resolve(root, process.env.CARGO_TARGET_DIR ?? 'target'),
	'debug/jewelcase-server'
);
const API = '/api/v1';

/**
 * This process's environment without its `JEWELCASE_` settings, which the test servers set for
 * themselves: one left in a developer's shell, such as a base path, would change or stop them.
 */
const inherited = Object.fromEntries(
	Object.entries(process.env).filter(([key]) => !key.startsWith('JEWELCASE_'))
);

/**
 * The environment variable that tells workers the run's folder, which `globalSetup` makes and
 * removes: the generated music is in `music` there, and each server's folders beside it.
 */
export const RUN_ENV = 'E2E_RUN_DIR';

/** The generated tracks, in the order they are added to a server's library. */
const TRACKS = [
	{
		title: 'Signal Part 1',
		album: 'Signal',
		artist: 'Aurora Lane',
		disc: '1/2',
		track: '1',
		genre: 'Ambient',
		mood: 'Calm'
	},
	{
		title: 'Signal Part 2',
		album: 'Signal',
		artist: 'Aurora Lane',
		disc: '1/2',
		track: '2',
		genre: 'Ambient'
	},
	{
		title: 'Signal Remix',
		album: 'Signal',
		artist: 'Aurora Lane',
		disc: '2/2',
		track: '1',
		genre: 'Electronic',
		mood: 'Calm'
	},
	{
		title: 'Kindling',
		album: 'Hearth',
		artist: 'Birch Hollow',
		disc: '1/1',
		track: '1',
		genre: 'Folk'
	}
].map((track) => ({ ...track, file: `${track.artist}/${track.album}/${track.title}.flac` }));

/** Part 1's synced lyrics, as the `.lrc` beside its file holds them. */
const LYRICS = '[00:00.00]First line of the song\n[00:01.00]Second line of the song\n';

/** The packed test plugin, which `buildServer` makes, for the tests to install. */
export const TEST_PLUGIN = join(root, 'web/e2e/test-plugin/target/e2e-test-plugin.wasm');

/**
 * Builds the debug server the tests run, then the test plugin, one cargo build after the other.
 * The web app the server serves is built by `webServer`.
 */
export function buildServer() {
	const env = { ...process.env, CARGO_BUILD_JOBS: '4' };
	execFileSync('cargo', ['build', '-q', '-p', 'jewelcase-server'], {
		cwd: root,
		env,
		stdio: 'inherit'
	});
	execFileSync(join(root, 'web/e2e/test-plugin/build.sh'), { env, stdio: 'inherit' });
}

/**
 * Writes the music into `dir` with ffmpeg: silent FLACs of 60 s, so no track ends during a test.
 * Album "Signal" by Aurora Lane has Part 1 (Ambient, Calm), Part 2 (Ambient) and, on disc 2,
 * Remix (Electronic, Calm); "Hearth" by Birch Hollow has one Folk track. Part 1 has a `.lrc`.
 */
export function generateMusic(dir: string) {
	for (const { file, artist, ...tags } of TRACKS) {
		const path = join(dir, file);
		mkdirSync(dirname(path), { recursive: true });
		const metadata = { ...tags, artist, album_artist: artist };
		const args = ['-v', 'error', '-f', 'lavfi', '-i', 'anullsrc=r=8000:cl=mono', '-t', '60'];
		for (const [key, value] of Object.entries(metadata)) args.push('-metadata', `${key}=${value}`);
		execFileSync('ffmpeg', [...args, '-c:a', 'flac', path]);
	}
	writeFileSync(join(dir, sidecar(TRACKS[0].file)), LYRICS);
}

/** The `.lrc` beside a track's file. */
const sidecar = (file: string) => file.replace(/\.flac$/, '.lrc');

/** A free TCP port on the loopback; another process could take it before use, which is rare. */
function freePort(): Promise<number> {
	return new Promise((done, fail) => {
		const probe = createServer().unref().on('error', fail);
		probe.listen(0, '127.0.0.1', () => {
			const { port } = probe.address() as { port: number };
			probe.close(() => done(port));
		});
	});
}

/** Polls `check` until it is truthy, or throws after `ms`. A throwing check counts as not yet. */
async function until(what: string, ms: number, check: () => Promise<unknown>) {
	const end = Date.now() + ms;
	while (!(await check().catch(() => false))) {
		if (Date.now() > end) throw new Error(`Timed out waiting for ${what}`);
		await new Promise((r) => setTimeout(r, 100));
	}
}

/** The IDs the server gave, looked up by name; it picks them at random. */
export interface Ids {
	library: string;
	tracks: Record<string, string>;
	albums: Record<string, string>;
	artists: Record<string, string>;
	tags: Record<string, string>;
}

export interface Server {
	/** Where the server, and so the web app it serves, answers. */
	url: string;
	/** The owner's login; the password is random per server. */
	username: string;
	password: string;
	ids: Ids;
	/** Calls the API as the owner, with a bearer token, and returns the JSON. Throws on a failing status. */
	// eslint-disable-next-line @typescript-eslint/no-explicit-any -- tests read the JSON they expect
	api: (path: string, init?: { method?: string; body?: unknown }) => Promise<any>;
	/** Stops the process and removes its folders. Safe to call twice. */
	stop: () => Promise<void>;
}

/** A server process that is running, not yet set up. */
export interface Launched {
	url: string;
	/** Where the server's own folders are: `music`, the `waiting` copy to add from, and `data`. */
	home: string;
	/** Stops the process and removes its folders. Safe to call twice. */
	stop: () => Promise<void>;
}

/**
 * Starts a server with its own data folder, on a free port, and waits until it answers. With
 * `withMusic`, its music folder holds Part 1, and the rest of the generated music (`RUN_ENV`) waits
 * in `home/waiting`; without it, the server has no music folder. Call `stop` when done, also on
 * failure.
 */
export async function launch(withMusic = true): Promise<Launched> {
	const home = mkdtempSync(join(process.env[RUN_ENV]!, 'server-'));
	let log = '';
	let child: ChildProcess | undefined;
	const stop = async () => {
		if (child && child.exitCode === null && child.signalCode === null) {
			const exited = new Promise((done) => child!.once('exit', done));
			child.kill('SIGTERM');
			const timer = setTimeout(() => child!.kill('SIGKILL'), 5000);
			await exited;
			clearTimeout(timer);
		}
		rmSync(home, { recursive: true, force: true });
	};
	try {
		const env = { ...inherited };
		if (withMusic) {
			mkdirSync(join(home, 'music'));
			cpSync(join(process.env[RUN_ENV]!, 'music'), join(home, 'waiting'), { recursive: true });
			moveIn(join(home, 'waiting'), join(home, 'music'), TRACKS[0].file);
			env.JEWELCASE_MUSIC = join(home, 'music');
		}
		const port = await freePort();
		const url = `http://127.0.0.1:${port}`;
		child = spawn(binary, [], {
			env: {
				...env,
				JEWELCASE_DATA_DIR: join(home, 'data'),
				JEWELCASE_PORT: String(port),
				JEWELCASE_WEB_DIR: join(root, 'web/build')
			},
			stdio: ['ignore', 'pipe', 'pipe']
		});
		for (const stream of [child.stdout!, child.stderr!]) {
			stream.on('data', (chunk) => (log = (log + chunk).slice(-4000)));
		}
		const died = new Promise<never>((_, fail) =>
			child!.once('exit', (code) => fail(new Error(`Server exited with ${code}:\n${log}`)))
		);
		await Promise.race([
			until('the server to answer', 60_000, () => fetch(`${url}${API}/health`).then((r) => r.ok)),
			died
		]);
		return { url, home, stop };
	} catch (error) {
		await stop();
		throw error;
	}
}

/** Completes a new server's setup with a random password, and returns the owner's login and token. */
export async function setupOwner(url: string) {
	const username = 'owner';
	const password = `e2e-${crypto.randomUUID()}`;
	const setup = await fetch(`${url}${API}/setup`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ username, password, device: { name: 'E2E harness', type: 'desktop' } })
	});
	if (setup.status !== 201) throw new Error(`Setup failed: ${setup.status} ${await setup.text()}`);
	const { token } = (await setup.json()) as { token: string };
	return { username, password, token };
}

/**
 * Starts a server, sets up its owner, and adds the music one track at a time, so each track's date
 * added differs as it does for real. Call `stop` when done, also on failure.
 */
export async function startServer(): Promise<Server> {
	const { url, home, stop } = await launch();
	const music = join(home, 'music');
	const waiting = join(home, 'waiting');
	try {
		const { username, password, token } = await setupOwner(url);

		const api: Server['api'] = async (path, { method = 'GET', body } = {}) => {
			const res = await fetch(`${url}${API}${path}`, {
				method,
				headers: {
					Authorization: `Bearer ${token}`,
					...(body !== undefined && { 'Content-Type': 'application/json' })
				},
				body: body === undefined ? undefined : JSON.stringify(body)
			});
			if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
			return res.status === 204 ? undefined : res.json();
		};

		const library = (await api('/libraries')).items[0].id as string;
		// Waits until the library holds `count` tracks and is not scanning.
		const scanned = (count: number) =>
			until(`${count} tracks to be scanned`, 30_000, async () => {
				const lib = await api(`/libraries/${library}`);
				return lib.trackCount === count && !lib.scanning;
			});
		await scanned(1);
		for (const [i, { file }] of TRACKS.slice(1).entries()) {
			moveIn(waiting, music, file);
			await scanned(i + 2);
		}

		// A search starts rebuilding a stale index and answers from the old one meanwhile; ask until
		// it knows the track added last, so every test finds the whole library.
		const last = TRACKS.at(-1)!.title;
		await until('the search index to hold the music', 30_000, async () => {
			const found = await api(`/libraries/${library}/search?q=${encodeURIComponent(last)}`);
			return found.top;
		});

		// Maps each item's `key` to its ID, for the first 100 items at `path` in the library.
		const byName = async (path: string, key: 'title' | 'name') =>
			Object.fromEntries(
				(await api(`/libraries/${library}${path}?limit=100`)).items.map(
					(item: Record<string, string>) => [item[key], item.id]
				)
			);
		const ids: Ids = {
			library,
			tracks: await byName('/tracks', 'title'),
			albums: await byName('/albums', 'title'),
			artists: await byName('/artists', 'name'),
			tags: await byName('/tags', 'name')
		};
		return { url, username, password, ids, api, stop };
	} catch (error) {
		await stop();
		throw error;
	}
}

/**
 * Moves a generated file, and the `.lrc` beside it if there is one, from one folder tree to the
 * same place in another.
 */
function moveIn(from: string, to: string, file: string) {
	mkdirSync(dirname(join(to, file)), { recursive: true });
	for (const name of [file, sidecar(file)]) {
		if (existsSync(join(from, name))) renameSync(join(from, name), join(to, name));
	}
}
