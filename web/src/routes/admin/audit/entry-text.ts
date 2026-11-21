import type { AuditEntry } from '$lib/api/types';

type Account = AuditEntry['target'];

/** An account as the entry named it, marked when it no longer exists. */
function account(a: Account, marked = true): string {
	const name = a.username ?? 'a deleted account';
	return marked && a.deleted && a.username ? `${name} (deleted)` : name;
}

/** `Music`, `Music and Audiobooks`, `Music, Audiobooks and Podcasts`. */
function join(names: string[]): string {
	return names.length < 2
		? names.join('')
		: `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

const ids = (value: unknown): string[] => (Array.isArray(value) ? (value as string[]) : []);

/**
 * The entry as a sentence naming who did what to whom. `libraryNames` names libraries by ID; one it
 * lacks reads as "a library".
 */
export function entryText(entry: AuditEntry, libraryNames: Map<string, string>): string {
	const { actor, target, detail } = entry;
	const by = actor ? account(actor) : 'someone';
	const to = account(target);
	const names = (value: unknown) =>
		join(ids(value).map((id) => libraryNames.get(id) ?? 'a library'));
	switch (entry.action) {
		case 'account_created':
			return actor ? `${to} joined with an invite from ${by}` : `${to} joined with an invite`;
		case 'access_changed': {
			const granted = ids(detail.granted).length > 0;
			const revoked = ids(detail.revoked).length > 0;
			const gave = granted ? `gave ${to} access to ${names(detail.granted)}` : '';
			const took = revoked
				? `took away ${granted ? '' : `${to}'s access to `}${names(detail.revoked)}`
				: '';
			return `${by} ${[gave, took].filter(Boolean).join(' and ') || `changed ${to}'s access`}`;
		}
		case 'suspended':
			return `${by} suspended ${to}`;
		case 'restored':
			return `${by} restored ${to}`;
		case 'devices_signed_out':
			return `${by} signed ${to} out of ${
				detail.all === true
					? 'every device'
					: ((detail.deviceName as string | undefined) ?? 'a device')
			}`;
		case 'password_reset_issued':
			return actor
				? `${by} made a password reset link for ${to}`
				: `a reset link for ${to} was made on the server`;
		case 'password_reset_used':
			return `someone used a reset link for ${to}`;
		case 'role_changed':
			return `${by} made ${to} ${detail.to === 'admin' ? 'an admin' : 'a user'}`;
		case 'ownership_transferred':
			return `${by} handed ownership to ${to}`;
		case 'deleted':
			// The target is always deleted, so the mark would say nothing.
			return `${by} deleted ${account(target, false)}`;
	}
}
