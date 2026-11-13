import { afterEach, expect, it, vi } from 'vitest';
import { FakeClock } from './fake-clock';
import { PROTOCOL, type CatalogState } from './protocol';
import { CatalogRouter, REPLY_IDLE_MS, Unanswered } from './router';

afterEach(() => vi.unstubAllGlobals());

/** A channel name of its own, so tests never hear each other. */
const channel = () => `router-${crypto.randomUUID()}`;

/** A state an owner could publish. */
const state = (serving = true): CatalogState => ({
	protocol: PROTOCOL,
	user: '7',
	serving,
	heavy: false,
	libraries: { '1': { onDevice: true, applying: false } }
});

/**
 * Resolves with the next message of `name` that `match` accepts, heard by a probe that is closed
 * after.
 */
function hear(name: string, match: (data: Record<string, unknown>) => boolean) {
	const probe = new BroadcastChannel(name);
	return new Promise<Record<string, unknown>>((resolve) => {
		probe.onmessage = ({ data }) => {
			if (!match(data)) return;
			probe.close();
			resolve(data);
		};
	});
}

it('tells every tab, the announcing one too, which library changed and in what kinds', async () => {
	const name = `changed-${crypto.randomUUID()}`;
	const here = new CatalogRouter(name);
	const there = new CatalogRouter(name);
	const heardHere = vi.fn();
	const heardThere = vi.fn();
	here.listen(heardHere);
	const stop = there.listen(heardThere);

	here.announce('1', ['liked', 'playlists']);
	expect(heardHere).toHaveBeenCalledWith('1', ['liked', 'playlists']);
	await vi.waitFor(() => expect(heardThere).toHaveBeenCalledWith('1', ['liked', 'playlists']));

	stop();
	here.announce('2', ['tags']);
	await vi.waitFor(() => expect(heardHere).toHaveBeenCalledWith('2', ['tags']));
	expect(heardThere).toHaveBeenCalledTimes(1);
});

it('answers only the tab that asked: on its reply channel, never on the shared one', async () => {
	const name = channel();
	const owner = new CatalogRouter(name);
	const asker = new CatalogRouter(name);
	const bystander = new CatalogRouter(name);
	const posted: { channel: string; type: string }[] = [];
	const post = BroadcastChannel.prototype.postMessage;
	vi.spyOn(BroadcastChannel.prototype, 'postMessage').mockImplementation(function (
		this: BroadcastChannel,
		message: { type: string }
	) {
		posted.push({ channel: this.name, type: message.type });
		return post.call(this, message);
	});
	// The bystander waits for the same kind of request, which it would settle if it heard an answer.
	const heardAnswer = vi.fn();
	const shared = new BroadcastChannel(name);
	shared.onmessage = ({ data }) => data.type === 'answer' && heardAnswer();
	owner.serve(async () => 'json');
	void bystander.ask({ type: 'query', library: '1', name: 'x', params: {} }).catch(() => {});

	expect(await asker.ask({ type: 'query', library: '1', name: 'x', params: {} })).toBe('json');
	const answers = posted.filter((p) => p.type === 'answer');
	expect(answers.length).toBeGreaterThanOrEqual(1);
	for (const answer of answers) expect(answer.channel).toMatch(/-reply-/);
	expect(heardAnswer).not.toHaveBeenCalled();
	shared.close();
});

it('keeps a reply channel open for the next answer and closes it once idle', async () => {
	const name = channel();
	const clock = new FakeClock();
	const opened: string[] = [];
	const closed: string[] = [];
	vi.stubGlobal(
		'BroadcastChannel',
		class extends BroadcastChannel {
			constructor(channelName: string) {
				super(channelName);
				opened.push(channelName);
			}
			close() {
				closed.push(this.name);
				super.close();
			}
		}
	);
	const owner = new CatalogRouter(name, undefined, undefined, clock);
	const asker = new CatalogRouter(name);
	owner.serve(async () => 'json');
	const base = opened.length;

	await asker.ask({ type: 'query', library: '1', name: 'x', params: {} });
	await asker.ask({ type: 'query', library: '1', name: 'x', params: {} });
	await asker.ask({ type: 'query', library: '1', name: 'x', params: {} });
	// One channel to the asker serves every answer, and stays open meanwhile.
	expect(opened.slice(base)).toHaveLength(1);
	expect(opened[base]).toMatch(/-reply-/);
	expect(closed).toEqual([]);

	await clock.advance(REPLY_IDLE_MS);
	expect(closed).toEqual([opened[base]]);
	expect(await asker.ask({ type: 'query', library: '1', name: 'x', params: {} })).toBe('json');
	expect(opened.slice(base)).toHaveLength(2);
});

it('a request nobody answers fails with Unanswered after its own timeout, on the clock it was given', async () => {
	const clock = new FakeClock();
	const router = new CatalogRouter(channel(), 5000, undefined, clock);
	const settled = vi.fn();
	const asked = router.ask({ type: 'query', library: '1', name: 'x', params: {} }, 100);
	asked.catch(settled);
	await clock.advance(99);
	expect(settled).not.toHaveBeenCalled();
	await clock.advance(1);
	await expect(asked).rejects.toBeInstanceOf(Unanswered);
});

it('a joining tab asks once and the owner tells it its state; a tab that does not serve does not answer', async () => {
	const name = channel();
	const owner = new CatalogRouter(name);
	const idle = new CatalogRouter(name);
	const joiner = new CatalogRouter(name);
	const told = vi.fn();
	joiner.onState(told);
	// Nothing is served yet: no one answers.
	idle.publish(state());
	await vi.waitFor(() => expect(told).toHaveBeenCalledTimes(1));
	told.mockClear();

	owner.serve(async () => '');
	owner.publish(state());
	await vi.waitFor(() => expect(told).toHaveBeenCalledWith(state(), false));
	told.mockClear();
	const probe = hear(name, (data) => data.type === 'probe');
	joiner.who();
	new BroadcastChannel(name).postMessage({ protocol: PROTOCOL, type: 'probe' });
	await vi.waitFor(() => expect(told).toHaveBeenCalledWith(state(), false));
	await probe;
	// Only the owner answered: the tab that published before it served stays quiet.
	expect(told).toHaveBeenCalledTimes(1);
});

it('a message of another protocol is no owner’s word: it settles nothing, tells nothing, and is noted', async () => {
	const name = channel();
	const clock = new FakeClock();
	const asker = new CatalogRouter(name, undefined, undefined, clock);
	const told = vi.fn();
	asker.onState(told);
	const old = new BroadcastChannel(name);
	// A version 1 owner answers on the shared channel with the ID of the request it heard.
	const request = hear(name, (data) => data.type === 'request');
	const outcome = asker.ask({ type: 'query', library: '1', name: 'x', params: {} }, 50).then(
		() => 'answered',
		(error) => error
	);
	const { id } = await request;
	old.postMessage({ type: 'answer', id, result: 'json' });
	old.postMessage({ type: 'state', state: state() });
	old.postMessage({ type: 'state', protocol: 1, state: state() });
	await vi.waitFor(() => expect(asker.foreign).toBe(true));
	expect(told).not.toHaveBeenCalled();
	// Its answer is not taken: the request waits out its time and fails.
	await clock.advance(50);
	expect(await outcome).toBeInstanceOf(Unanswered);
	old.close();
});

it('an owner ignores a request of another protocol, as from a tab of an older build', async () => {
	const name = channel();
	const owner = new CatalogRouter(name);
	const asker = new CatalogRouter(name);
	const handled = vi.fn(async () => 'json');
	owner.serve(handled);
	const old = new BroadcastChannel(name);
	old.postMessage({
		type: 'request',
		id: 'old',
		request: { type: 'query', library: '1', name: 'x', params: {} }
	});
	// Heard in order, so once this one is answered the old one has been dealt with.
	expect(await asker.ask({ type: 'query', library: '2', name: 'x', params: {} })).toBe('json');
	expect(handled).toHaveBeenCalledTimes(1);
	expect(handled).toHaveBeenCalledWith({ type: 'query', library: '2', name: 'x', params: {} });
	expect(owner.foreign).toBe(true);
	old.close();
});
