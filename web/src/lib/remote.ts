// Carrying out commands from the user's other devices (`requirements/realtime.md` §3). The
// server sends one only to the connection playing, which carries it out and says whether it
// did, so the device that sent it shows what really happened (§5).

import type { PlayerCommand } from './api/types';

/** What of the player a command can reach. */
export interface Controls {
	/** Whether a track is loaded. */
	loaded(): boolean;
	playing(): boolean;
	/** Plays the current track, settling once the audio plays, or failing if it cannot. */
	resume(): Promise<void>;
	pause(): void;
	next(): void;
	previous(): void;
	seek(seconds: number): void;
	setVolume(volume: number): void;
}

/** Carries `command` out on `player`, and answers why it could not, or null if it did. */
export async function carryOut(player: Controls, command: PlayerCommand): Promise<string | null> {
	if (command.type === 'setVolume') {
		player.setVolume(command.volume);
		return null;
	}
	if (!player.loaded()) return 'Nothing is loaded on this device.';
	switch (command.type) {
		case 'play':
			if (player.playing()) return null;
			// A browser may refuse to start audio a page did not start itself. A pause right
			// after cuts the play short, which is the last command winning, not a failure.
			return player.resume().then(
				() => null,
				(error) => (error?.name === 'AbortError' ? null : 'This device could not start playing.')
			);
		case 'pause':
			player.pause();
			return null;
		case 'next':
			player.next();
			return null;
		case 'previous':
			player.previous();
			return null;
		case 'seek':
			player.seek(command.positionMs / 1000);
			return null;
		default:
			return 'This device cannot do that yet.';
	}
}
