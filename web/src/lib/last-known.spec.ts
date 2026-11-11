import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
	claim,
	forget,
	lastKnown,
	logoutPending,
	owner,
	remember,
	rememberDevice,
	setLogoutPending
} from './last-known';

/**
 * The browser storage the module reads and writes: its items are the object's own properties, as a
 * real storage's are.
 */
let items: Record<string, string>;

beforeEach(() => {
	items = Object.defineProperties({} as Record<string, string>, {
		getItem: { value: (key: string) => items[key] ?? null },
		setItem: { value: (key: string, value: string) => void (items[key] = value) },
		removeItem: { value: (key: string) => void delete items[key] }
	});
	vi.stubGlobal('localStorage', items);
});

describe('what the device keeps of who is signed in', () => {
	it('keeps each account’s user and libraries apart, and reads only the owner’s', () => {
		remember({ me: { id: '1' } as never });
		remember({ me: { id: '2' } as never });
		claim('1');
		remember({ libraries: { items: [{ id: '9' } as never] } });

		expect(lastKnown().me).toEqual({ id: '1' });
		expect(lastKnown().libraries).toEqual({ items: [{ id: '9' }] });
		claim('2');
		expect(lastKnown().me).toEqual({ id: '2' });
		expect(lastKnown().libraries).toBeUndefined();
	});

	it('writes a user under that user’s own ID, whoever owns the device', () => {
		claim('1');
		remember({ me: { id: '2' } as never });
		expect(items['jewelcase-user:1']).toBeUndefined();
		expect(items['jewelcase-user:2']).toBe('{"me":{"id":"2"}}');
	});

	it('forgetting deletes every account’s part and the owner, but not the server’s details', () => {
		remember({ server: { setupRequired: false } as never, me: { id: '1' } as never });
		remember({ me: { id: '2' } as never });
		claim('1');
		rememberDevice('1', ['a']);

		forget();

		expect(owner()).toBeNull();
		expect(Object.keys(items)).toEqual(['jewelcase-server']);
	});

	it('keeps the marker of a logout the server has not heard of apart from the rest, which forgetting leaves', () => {
		expect(logoutPending()).toBe(false);
		setLogoutPending(true);
		forget();
		expect(logoutPending()).toBe(true);
		setLogoutPending(false);
		expect(logoutPending()).toBe(false);
	});
});
