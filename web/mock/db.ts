// Read-only access to the database the real scanner writes. The mock never
// writes: the scanner is the only ingestion path (`requirements/general.md` §3.2).

import { existsSync } from 'node:fs';
import { DatabaseSync, type SQLInputValue } from 'node:sqlite';

export type Row = Record<string, unknown>;

/**
 * IDs are random 63-bit integers (`design/database.md` §1), past JavaScript's
 * safe-integer range, so every statement reads integers as `bigint`. `id()`
 * turns one into the API's opaque string and `toKey()` turns it back.
 */
export class Db {
	#db: DatabaseSync | null = null;

	constructor(readonly path: string) {}

	/** Null until the file exists, so the mock serves an empty library before the first scan. */
	#open(): DatabaseSync | null {
		if (!this.#db && existsSync(this.path)) {
			this.#db = new DatabaseSync(this.path, { readOnly: true });
		}
		return this.#db;
	}

	get available(): boolean {
		return this.#open() !== null;
	}

	all(sql: string, ...params: SQLInputValue[]): Row[] {
		const db = this.#open();
		if (!db) return [];
		const stmt = db.prepare(sql);
		stmt.setReadBigInts(true);
		return stmt.all(...params) as Row[];
	}

	get(sql: string, ...params: SQLInputValue[]): Row | undefined {
		return this.all(sql, ...params)[0];
	}
}

export function id(v: unknown): string {
	return String(v);
}

/** An API ID back to a database key; `null` for anything that cannot be one, which answers 404. */
export function toKey(s: string | null | undefined): bigint | null {
	if (!s || !/^\d{1,19}$/.test(s)) return null;
	return BigInt(s);
}

export function num(v: unknown): number {
	return Number(v);
}

export function numOrNull(v: unknown): number | null {
	return v === null || v === undefined ? null : Number(v);
}

export function str(v: unknown): string | null {
	return v === null || v === undefined ? null : String(v);
}

export function iso(ms: unknown): string {
	return new Date(Number(ms)).toISOString();
}
