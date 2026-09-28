// Artwork, audio, waveforms, and playback negotiation for the mock API.

import { spawn } from 'node:child_process';
import { createReadStream, statSync } from 'node:fs';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { join } from 'node:path';
import type { PlaybackInfo } from '../src/lib/api/types.ts';
import type { Catalog } from './catalog.ts';
import { type Db, type Row, num, numOrNull } from './db.ts';
import { playlist } from './playlists.ts';
import { toApiWaveform } from './waveform.ts';

const FFMPEG = process.env.FFMPEG ?? 'ffmpeg';

// ───────────────────────────── Images ─────────────────────────────

/** The server rounds requested sizes up to a fixed set (`ImageSize` in the API). */
const SIZES = [64, 128, 256, 512, 1024, 2048];

export function imageSize(requested: string | null): number {
	const n = Number(requested) || 512;
	return SIZES.find((s) => s >= n) ?? SIZES[SIZES.length - 1];
}

const imageCache = new Map<string, Promise<Buffer | null>>();

/** A library image or playlist mosaic as JPEG, scaled to `size`; null when it does not exist. */
export function image(db: Db, lib: bigint, imageId: string, size: number): Promise<Buffer | null> {
	const key = `${lib}/${imageId}@${size}`;
	let hit = imageCache.get(key);
	if (!hit) {
		hit = render(db, lib, imageId, size);
		imageCache.set(key, hit);
		// Missing images are not cached, so a later scan can fill them in.
		hit.then((b) => b ?? imageCache.delete(key));
	}
	return hit;
}

function imagePath(db: Db, lib: bigint, imageId: bigint): string | null {
	const row = db.get(
		`SELECT i.path, r.path AS root FROM images i JOIN library_roots r ON r.id = i.root_id
		 WHERE i.library_id = ? AND i.id = ?`,
		lib,
		imageId
	);
	// An embedded image's path is its track file; ffmpeg reads the attached picture from either.
	return row ? join(String(row.root), String(row.path)) : null;
}

async function render(db: Db, lib: bigint, imageId: string, size: number): Promise<Buffer | null> {
	if (imageId.startsWith('mosaic-')) {
		const list = playlist(db, lib, imageId.slice('mosaic-'.length));
		if (!list) return null;
		const covers = new Set<string>();
		for (const t of list.trackIds) {
			const row = db.get(
				'SELECT al.image_id FROM tracks t JOIN albums al ON al.id = t.album_id WHERE t.id = ?',
				t
			);
			const path = row?.image_id ? imagePath(db, lib, row.image_id as bigint) : null;
			if (path) covers.add(path);
			if (covers.size === 4) break;
		}
		return mosaic([...covers], size);
	}
	if (!/^\d{1,19}$/.test(imageId)) return null;
	const path = imagePath(db, lib, BigInt(imageId));
	if (!path) return null;
	return ffmpeg([
		'-i',
		path,
		'-map',
		'0:v:0',
		'-frames:v',
		'1',
		'-vf',
		`scale=${size}:${size}:force_original_aspect_ratio=decrease`
	]);
}

/** Four covers in a 2×2 grid, or one cover filling the square when there are fewer. */
function mosaic(covers: string[], size: number): Promise<Buffer | null> {
	if (covers.length === 0) return Promise.resolve(null);
	const fill = (s: number) => `scale=${s}:${s}:force_original_aspect_ratio=increase,crop=${s}:${s}`;
	if (covers.length < 4) {
		return ffmpeg(['-i', covers[0], '-map', '0:v:0', '-frames:v', '1', '-vf', fill(size)]);
	}
	const half = Math.floor(size / 2);
	const graph =
		covers.map((_, i) => `[${i}:v:0]${fill(half)}[c${i}]`).join(';') +
		';[c0][c1][c2][c3]xstack=inputs=4:layout=0_0|w0_0|0_h0|w0_h0';
	return ffmpeg([...covers.flatMap((c) => ['-i', c]), '-filter_complex', graph, '-frames:v', '1']);
}

function ffmpeg(args: string[]): Promise<Buffer | null> {
	return new Promise((resolve) => {
		const proc = spawn(FFMPEG, [
			'-loglevel',
			'error',
			...args,
			'-f',
			'image2pipe',
			'-c:v',
			'mjpeg',
			'-q:v',
			'3',
			'pipe:1'
		]);
		const chunks: Buffer[] = [];
		proc.stdout.on('data', (c: Buffer) => chunks.push(c));
		proc.on('error', () => resolve(null));
		proc.on('close', (code) => resolve(code === 0 && chunks.length ? Buffer.concat(chunks) : null));
	});
}

// ───────────────────────────── Audio ─────────────────────────────

const CONTENT_TYPES: Record<string, string> = {
	mp3: 'audio/mpeg',
	flac: 'audio/flac',
	mp4: 'audio/mp4',
	ogg: 'audio/ogg',
	wav: 'audio/wav',
	aiff: 'audio/aiff',
	matroska: 'audio/webm',
	webm: 'audio/webm'
};

export function playbackInfo(track: Row): PlaybackInfo {
	const lufs = numOrNull(track.loudness_lufs);
	return {
		// The mock always delivers the original; negotiation and transcoding are the server's to build.
		variant: 'original',
		delivery: 'direct',
		codec: String(track.codec),
		container: String(track.container),
		bitrateKbps: numOrNull(track.bitrate_kbps),
		sampleRateHz: num(track.sample_rate_hz),
		bitDepth: numOrNull(track.bit_depth),
		channels: num(track.channels),
		sizeBytes: num(track.file_size),
		transport: 'progressive',
		loudness:
			lufs === null
				? null
				: {
						trackLufs: lufs,
						trackPeakDbtp: num(track.peak_dbtp),
						albumLufs: numOrNull(track.album_lufs),
						albumPeakDbtp: numOrNull(track.album_peak)
					},
		gapless: null
	};
}

/** Streams the track's file, honouring a single byte range for seeking. */
export function streamAudio(track: Row, req: IncomingMessage, res: ServerResponse): void {
	const path = join(String(track.root_path), String(track.path));
	let size: number;
	try {
		size = statSync(path).size;
	} catch {
		res.statusCode = 404;
		res.end();
		return;
	}
	res.setHeader('Accept-Ranges', 'bytes');
	res.setHeader(
		'Content-Type',
		CONTENT_TYPES[String(track.container)] ?? 'application/octet-stream'
	);
	const range = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range ?? '');
	let start = 0;
	let end = size - 1;
	if (range) {
		if (range[1] === '') {
			start = Math.max(0, size - Number(range[2]));
		} else {
			start = Number(range[1]);
			if (range[2] !== '') end = Math.min(end, Number(range[2]));
		}
		if (start > end || start >= size) {
			res.statusCode = 416;
			res.setHeader('Content-Range', `bytes */${size}`);
			res.end();
			return;
		}
		res.statusCode = 206;
		res.setHeader('Content-Range', `bytes ${start}-${end}/${size}`);
	}
	res.setHeader('Content-Length', end - start + 1);
	if (req.method === 'HEAD') {
		res.end();
		return;
	}
	createReadStream(path, { start, end }).pipe(res);
}

// ───────────────────────────── Waveforms ─────────────────────────────

/** The track's waveform, or null when it has not been analyzed yet (the API's `204`). */
export function waveform(catalog: Catalog, lib: bigint, trackId: bigint) {
	const row = catalog.db.get(
		'SELECT data FROM track_waveforms WHERE library_id = ? AND track_id = ?',
		lib,
		trackId
	);
	return row ? toApiWaveform(row.data as Uint8Array) : null;
}
