// Brief messages at the bottom of the main area, with at most one action, such as Undo
// (`requirements/playlists.md` §4). One shows at a time, and a new one replaces it: a stack
// of them would cover the page on a phone.

export type ToastOptions = {
	action?: { label: string; run: () => void };
	/** Milliseconds shown, not counting while it is pointed at or has focus. */
	duration?: number;
	/** When it goes without its action taken: timed out, dismissed, or replaced. */
	onclose?: () => void;
};

type Toast = ToastOptions & { id: number; message: string };

export const TOAST_MS = 6000;

class Toaster {
	current = $state<Toast | null>(null);

	#next = 0;
	#timer: ReturnType<typeof setTimeout> | undefined;
	/** Time left on the current toast, as of `#since`. */
	#left = 0;
	#since = 0;
	/** Pointer and focus each hold the toast open while they are on it. */
	#holds = 0;

	/** Shows `message`, replacing whatever was showing. Returns a function that dismisses it. */
	show(message: string, options: ToastOptions = {}): () => void {
		this.dismiss();
		const toast: Toast = { ...options, id: ++this.#next, message };
		this.current = toast;
		this.#left = options.duration ?? TOAST_MS;
		this.#holds = 0;
		this.#run();
		return () => {
			if (this.current?.id === toast.id) this.dismiss();
		};
	}

	/** Closes it without its action. */
	dismiss() {
		const toast = this.#end();
		toast?.onclose?.();
	}

	/** Closes it and takes its action. */
	act() {
		const toast = this.#end();
		toast?.action?.run();
	}

	/** Stops the clock while something is on the toast. */
	hold() {
		if (!this.current) return;
		if (this.#holds++ === 0) {
			clearTimeout(this.#timer);
			this.#left -= Date.now() - this.#since;
		}
	}

	release() {
		if (!this.current || this.#holds === 0) return;
		if (--this.#holds === 0) this.#run();
	}

	#run() {
		this.#since = Date.now();
		this.#timer = setTimeout(() => this.dismiss(), Math.max(0, this.#left));
	}

	#end(): Toast | null {
		const toast = this.current;
		clearTimeout(this.#timer);
		this.current = null;
		return toast;
	}
}

export const toaster = new Toaster();

/** Shows `message`, replacing whatever was showing. Returns a function that dismisses it. */
export const toast = (message: string, options?: ToastOptions) => toaster.show(message, options);
