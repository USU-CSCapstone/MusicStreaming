import type { Page, Request } from '@playwright/test';
import type { PlayReport, RealtimeClientMessage, SentEdit, StartQueue } from '../src/lib/api/types';
import { type Server, setupOwner } from './server';
import { createPlaylist, expect, login, test } from './test';

// The queue against a real server: the page's starts and edits are read off its requests, what
// the server made of them off its API.

type Progress = Extract<RealtimeClientMessage, { type: 'progress' }>['report'];

/**
 * Records what the page sends from now on: queue starts and edits as posted, plays, restores, and
 * the progress reports on the realtime connection.
 */
function observe(page: Page) {
	const sent = {
		starts: [] as StartQueue[],
		edits: [] as SentEdit[],
		plays: [] as PlayReport[],
		restores: [] as string[],
		reports: [] as Progress[]
	};
	page.on('request', (request) => {
		if (request.method() !== 'POST') return;
		const path = new URL(request.url()).pathname.replace('/api/v1', '');
		if (path === '/me/queue') sent.starts.push(request.postDataJSON());
		else if (path === '/me/queue/edits') sent.edits.push(request.postDataJSON().edit);
		else if (path === '/me/plays') sent.plays.push(...request.postDataJSON().items);
		else if (path.startsWith('/me/queues/')) sent.restores.push(path.split('/')[3]);
	});
	page.on('websocket', (socket) =>
		socket.on('framesent', ({ payload }) => {
			const message = JSON.parse(String(payload));
			if (message.type === 'progress') sent.reports.push(message.report);
		})
	);
	return sent;
}

/** The IDs of the tracks titled `titles`, in that order. */
const ids = (server: Server, ...titles: string[]) => titles.map((t) => server.ids.tracks[t]);

/** The track IDs of each list the active queue holds, in the order its sources were added. */
async function lists(server: Server): Promise<string[][]> {
	const { state } = await server.api('/me/queue');
	return Promise.all(
		state.sources.map(
			async ({ list }: { list: string }) => (await server.api(`/me/queue/lists/${list}`)).tracks
		)
	);
}

/** The ID of the account's active queue, or null before it has one. */
const activeQueue = (server: Server) =>
	server.api('/me/queue').then(
		(q) => q.queueId as string,
		() => null
	);

/** The IDs of the account's kept queues, newest first. */
async function kept(server: Server): Promise<string[]> {
	return (await server.api('/me/queues')).items.map((q: { queueId: string }) => q.queueId);
}

/**
 * Holds the requests that `match` in `page`, as a slow server would, until the returned function
 * is called; the others go on.
 */
async function hold(page: Page, url: string | RegExp, match: (r: Request) => boolean = () => true) {
	const release = Promise.withResolvers<void>();
	await page.route(url, async (route) => {
		if (match(route.request())) await release.promise;
		await route.continue();
	});
	return release.resolve;
}

/** Whether `r` is a queue start, the only POST to the queue's own URL. */
const isStart = (r: Request) => r.method() === 'POST';
/** The player bar. */
const player = (page: Page) => page.getByRole('region', { name: 'Player' });
/** The queue panel. */
const panelOf = (page: Page) => page.getByRole('complementary', { name: 'Queue' });
/** The page's main content. */
const main = (page: Page) => page.locator('main');
/** The Play button of the page's main content. */
const playButton = (page: Page) => main(page).getByRole('button', { name: 'Play', exact: true });
/** The titles of the tracks listed in the page's main content. */
const shownTitles = (page: Page) => main(page).locator('.track .title');

/** Starts the album on its first track, with the queue panel open. */
async function openUpNext(page: Page, server: Server) {
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await playButton(page).click();
	await expect(player(page).getByText('Signal Part 1')).toBeVisible();
	await player(page).getByRole('button', { name: 'Queue' }).click();
	const panel = panelOf(page);
	return { panel, list: panel.getByRole('list', { name: 'Up Next' }) };
}

/**
 * Waits, in the queue panel, until a page just loaded has fetched the queue again, then closes
 * the panel. Queueing before the page has a queue only says to start something first.
 */
async function queueLoaded(page: Page) {
	await player(page).getByRole('button', { name: 'Queue' }).click();
	const panel = panelOf(page);
	await expect(panel.getByRole('tab').first()).toHaveText(/Up Next [1-9]/);
	await panel.getByRole('button', { name: 'Close queue' }).click();
}

/** Plays the album from its page, and gives the player, the queue panel and Up Next's titles. */
async function playAlbum(page: Page, server: Server) {
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await playButton(page).click();
	await expect(player(page).getByText('Signal Part 1')).toBeVisible();
	const panel = panelOf(page);
	const manual = () =>
		panel.getByRole('list', { name: 'Up Next' }).locator('[data-item] .title').allInnerTexts();
	return { player: player(page), panel, manual };
}

/**
 * `titles` in the order the server sorts by when they were added, which for tracks added together
 * is by ID.
 */
const byId = (server: Server, titles: string[]) =>
	titles.toSorted((a, b) => (BigInt(server.ids.tracks[a]) < BigInt(server.ids.tracks[b]) ? -1 : 1));

test('plays an album from the queue', async ({ page, server }) => {
	const { Signal } = server.ids.albums;
	const sent = observe(page);
	// The server answers the start only once the test lets it: the album plays before that.
	const answer = await hold(page, '**/api/v1/me/queue', isStart);
	const before = await activeQueue(server);

	await page.goto(`/albums/${Signal}`);
	await playButton(page).click();
	await expect(player(page).getByText('Signal Part 1')).toBeVisible();
	await expect(player(page).getByRole('button', { name: 'Pause' })).toBeVisible();
	await expect.poll(() => sent.plays.length).toBeGreaterThan(0);
	expect(sent.starts, 'sent').toHaveLength(1);
	expect(await activeQueue(server), 'not answered yet').toBe(before);

	answer();
	await expect.poll(() => activeQueue(server)).toBe(sent.starts[0].queueId);
	// The album's recipe, with the row clicked and how many rows the view has, not its tracks.
	expect(sent.starts[0]).toMatchObject({
		context: { type: 'album', id: Signal },
		source: {
			libraryId: server.ids.library,
			recipe: { type: 'album', albumId: Signal, sort: 'album', order: 'asc' }
		},
		start: 0,
		startTrackId: server.ids.tracks['Signal Part 1']
	});
	expect(sent.starts[0].source).not.toHaveProperty('tracks');

	// Next moves the queue on, and plays what it moves to.
	await player(page).getByRole('button', { name: 'Next' }).click();
	await expect(player(page).getByText('Signal Part 2')).toBeVisible();
	await player(page).getByRole('button', { name: 'Next' }).click();
	await expect(player(page).getByText('Signal Remix')).toBeVisible();
	await expect.poll(() => sent.edits).toEqual([{ type: 'next' }, { type: 'next' }]);

	// Every play records the album it was played from.
	await expect
		.poll(() => sent.plays.find((p) => p.trackId === server.ids.tracks['Signal Remix']))
		.toMatchObject({ context: { type: 'album', id: Signal }, origin: 'context' });
});

test('a tag-filtered view starts by a recipe carrying its filter', async ({ page, server }) => {
	const { Ambient } = server.ids.tags;
	const sent = observe(page);
	await page.goto(`/songs?tagAny=${Ambient}`);
	await expect(shownTitles(page)).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await page.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
	await expect(player(page).getByText('Signal Part 1')).toBeVisible();
	await expect.poll(() => sent.starts.length).toBe(1);
	// The start names the filter, not the songs, and the server's resolve is the rows shown.
	expect(sent.starts[0]).toMatchObject({
		source: { recipe: { type: 'library', filters: { tagAny: [Ambient] } } },
		startTrackId: server.ids.tracks['Signal Part 1'],
		context: { type: 'filter', filter: { tagAny: [Ambient] } }
	});
	expect(sent.starts[0].source).not.toHaveProperty('tracks');
	await expect
		.poll(async () => (await lists(server)).at(-1))
		.toEqual(ids(server, 'Signal Part 2', 'Signal Part 1'));
});

test('reorders and removes in Up Next', async ({ page, server }) => {
	const sent = observe(page);
	const { panel, list } = await openUpNext(page, server);
	const titles = () => list.locator('[data-item] .title').allInnerTexts();
	await expect.poll(titles).toEqual(['Signal Part 2', 'Signal Remix']);
	await expect(panel.getByText('Nothing queued.')).toBeVisible();

	// Dragging the grip of the first row below the last moves it to the end of the context.
	const grip = list.locator('[data-item]').first().locator('.grip');
	const [from, last] = [
		await grip.boundingBox(),
		await list.locator('[data-item]').last().boundingBox()
	];
	await page.mouse.move(from!.x + 5, from!.y + 5);
	await page.mouse.down();
	await page.mouse.move(from!.x + 5, from!.y + 40, { steps: 4 });
	await page.mouse.move(from!.x + 5, last!.y + last!.height - 4, { steps: 4 });
	await page.mouse.up();
	await expect.poll(titles).toEqual(['Signal Remix', 'Signal Part 2']);
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({ type: 'move', before: { type: 'contextEnd' } });

	// Alt+↑ from the keyboard moves the last row up, and up again across the Manual Queue's end.
	await list.getByRole('button', { name: 'Signal Part 2', exact: true }).focus();
	await page.keyboard.press('Alt+ArrowUp');
	await expect.poll(titles).toEqual(['Signal Part 2', 'Signal Remix']);
	await page.keyboard.press('Alt+ArrowUp');
	await expect(panel.getByRole('heading', { name: /Queued by you\s*1/ })).toBeVisible();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({ type: 'move', before: { type: 'manualEnd' } });
	await expect(panel.getByText('Nothing queued.')).toBeHidden();

	// One press removes a row from anywhere, the Manual Queue's included.
	await panel.getByRole('button', { name: 'Remove Signal Part 2' }).click();
	await expect.poll(titles).toEqual(['Signal Remix']);
	await expect.poll(() => sent.edits.at(-1)).toMatchObject({ type: 'remove' });
	await expect(panel.getByText('Nothing queued.')).toBeVisible();

	// Clicking a row plays it, the rows before it going to History.
	await list.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await expect(player(page).getByText('Signal Remix')).toBeVisible();
	await expect.poll(() => sent.edits.at(-1)).toMatchObject({ type: 'jump' });
});

test('reaches Shuffle and Repeat from the panel at phone width', async ({ page, server }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	const sent = observe(page);
	const { panel } = await openUpNext(page, server);
	// The player bar has no room for them.
	await expect(player(page).getByRole('button', { name: 'Shuffle' })).toBeHidden();
	await panel.getByRole('button', { name: 'Shuffle' }).click();
	await expect(panel.getByRole('button', { name: 'Shuffle' })).toHaveAttribute(
		'aria-pressed',
		'true'
	);
	await panel.getByRole('button', { name: 'Repeat off' }).click();
	await expect(panel.getByRole('button', { name: 'Repeat all' })).toBeVisible();
	await expect
		.poll(() => sent.edits.map((e) => e.type))
		.toEqual(expect.arrayContaining(['shuffle', 'repeat']));
	expect(sent.edits.at(-1)).toMatchObject({ type: 'repeat', mode: 'all' });
});

/** Plays the album, skips `nexts` tracks, then opens the queue panel and its History tab. */
async function openHistory(page: Page, server: Server, nexts: number) {
	const { panel, list } = await openUpNext(page, server);
	for (let i = 0; i < nexts; i++) await player(page).getByRole('button', { name: 'Next' }).click();
	await panel.getByRole('tab', { name: /History/ }).click();
	return {
		panel,
		player: player(page),
		list,
		history: panel.getByRole('list', { name: 'History' })
	};
}

test('rewinds from History', async ({ page, server }) => {
	const sent = observe(page);
	const { panel, player, history } = await openHistory(page, server, 2);
	await expect(player.getByText('Signal Remix')).toBeVisible();
	const titles = () => history.locator('[data-item] .title').allInnerTexts();
	// Newest first.
	await expect.poll(titles).toEqual(['Signal Part 2', 'Signal Part 1']);

	// Choosing the older play goes back to it, as Previous twice would.
	await history.getByRole('button', { name: 'Signal Part 1', exact: true }).click();
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({ type: 'rewind', play: { play: 0 }, key: expect.any(String) });
	await expect(panel.getByText('Nothing has played yet.')).toBeVisible();
	await panel.getByRole('tab', { name: /Up Next/ }).click();
	const upNext = panel.getByRole('list', { name: 'Up Next' });
	await expect
		.poll(() => upNext.locator('[data-item] .title').allInnerTexts())
		.toEqual(['Signal Part 2', 'Signal Remix']);
});

test('re-queues a played track', async ({ page, server }) => {
	const sent = observe(page);
	const { panel, player, history } = await openHistory(page, server, 1);
	await expect(player.getByText('Signal Part 2')).toBeVisible();
	await expect(history.getByRole('button', { name: 'Signal Part 1', exact: true })).toBeVisible();

	await panel.getByRole('button', { name: 'Queue Signal Part 1 next' }).click();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({ type: 'requeue', play: { play: 0 }, key: expect.any(String) });
	await expect(panel.getByText('Nothing has played yet.')).toBeVisible();

	// It is at the front of Up Next, in the Manual Queue, and the song playing is undisturbed.
	await panel.getByRole('tab', { name: /Up Next/ }).click();
	const upNext = panel.getByRole('list', { name: 'Up Next' });
	await expect(panel.getByRole('heading', { name: /Queued by you\s*1/ })).toBeVisible();
	await expect
		.poll(() => upNext.locator('[data-item] .title').allInnerTexts())
		.toEqual(['Signal Part 1', 'Signal Remix']);
	await expect(player.getByText('Signal Part 2')).toBeVisible();
});

test('queues an album next', async ({ page, server }) => {
	const { Signal } = server.ids.albums;
	const sent = observe(page);
	const { player, panel } = await playAlbum(page, server);

	await main(page).getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue next' }).click();
	await expect(page.getByRole('status').getByText('“Signal” queued next')).toBeVisible();
	// The album's recipe goes, not its tracks, at the Manual Queue's front.
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: {
				libraryId: server.ids.library,
				recipe: { type: 'album', albumId: Signal, sort: 'album', order: 'asc' }
			},
			before: { type: 'manualStart' }
		});
	expect(sent.edits.at(-1)).not.toHaveProperty('source.tracks');

	await player.getByRole('button', { name: 'Queue' }).click();
	await expect(panel.getByRole('heading', { name: /Queued by you\s*3/ })).toBeVisible();
	await expect
		.poll(() =>
			panel.getByRole('list', { name: 'Up Next' }).locator('[data-item] .title').allInnerTexts()
		)
		.toEqual(['Signal Part 1', 'Signal Part 2', 'Signal Remix', 'Signal Part 2', 'Signal Remix']);
});

test('queues a song last', async ({ page, server }) => {
	const sent = observe(page);
	const { player, panel, manual } = await playAlbum(page, server);

	await main(page).getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect(page.getByRole('status').getByText('“Signal Remix” queued last')).toBeVisible();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { libraryId: server.ids.library, tracks: ids(server, 'Signal Remix') },
			before: { type: 'manualEnd' }
		});
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect(panel.getByRole('heading', { name: /Queued by you\s*1/ })).toBeVisible();
	await expect.poll(manual).toEqual(['Signal Remix', 'Signal Part 2', 'Signal Remix']);
});

test('drops a song into Up Next where it lands', async ({ page, server }) => {
	const sent = observe(page);
	const { player, panel, manual } = await playAlbum(page, server);
	await player.getByRole('button', { name: 'Queue' }).click();
	await expect.poll(manual).toEqual(['Signal Part 2', 'Signal Remix']);

	// Dragged from the album's row onto the gap above the context's last row.
	const row = main(page).locator('li.track', { hasText: 'Signal Part 1' }).first();
	const last = panel.locator('[data-item]').last();
	const box = (await last.boundingBox())!;
	await row.dragTo(panel.getByRole('list', { name: 'Up Next' }), {
		targetPosition: {
			x: 80,
			y: box.y - (await panel.getByRole('list', { name: 'Up Next' }).boundingBox())!.y + 2
		}
	});
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { tracks: ids(server, 'Signal Part 1') },
			before: { type: 'before', item: expect.stringMatching(/\.2$/) }
		});
	await expect.poll(manual).toEqual(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);
});

test('adds to a chosen point of Up Next from the keyboard', async ({ page, server }) => {
	const sent = observe(page);
	const { panel, manual } = await playAlbum(page, server);
	await main(page).getByRole('button', { name: 'Actions for Signal Part 1' }).focus();
	await page.keyboard.press('Enter');
	await page.getByRole('menuitem', { name: 'Add to Up Next…' }).click();

	// The panel opens to choose where: Escape would give it up.
	const choose = panel.getByRole('group', { name: 'Choose where to add' });
	await expect(choose).toBeVisible();
	await expect(choose.getByRole('button', { name: 'Add to the end' })).toBeFocused();
	await panel
		.getByRole('button', { name: 'Add “Signal Part 1” before Signal Remix' })
		.press('Enter');
	await expect(choose).toBeHidden();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { tracks: ids(server, 'Signal Part 1') },
			before: { type: 'before', item: expect.stringMatching(/\.2$/) }
		});
	await expect.poll(manual).toEqual(['Signal Part 2', 'Signal Part 1', 'Signal Remix']);
	// Focus stays on the row it was added before.
	await expect(panel.getByRole('button', { name: 'Signal Remix', exact: true })).toBeFocused();
});

test('adds an album to a chosen point of Up Next from the keyboard', async ({ page, server }) => {
	const { Signal } = server.ids.albums;
	const sent = observe(page);
	const { panel } = await playAlbum(page, server);
	await main(page).getByRole('button', { name: 'Actions for “Signal”' }).focus();
	await page.keyboard.press('Enter');
	await page.getByRole('menuitem', { name: 'Add to Up Next…' }).click();

	const choose = panel.getByRole('group', { name: 'Choose where to add' });
	await choose.getByRole('button', { name: 'Add to the end' }).press('Enter');
	await expect(choose).toBeHidden();
	// The recipe is posted to the worker, which cannot take what the page's state wraps.
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'album', albumId: Signal } }
		});
});

test('queues a sorted playlist by a recipe that resolves to the rows shown', async ({
	page,
	server
}) => {
	const sent = observe(page);
	const mix = await createPlaylist(server, 'Signal Mix', [
		'Signal Part 1',
		'Signal Part 2',
		'Signal Remix'
	]);
	await page.goto(`/playlists/${mix}?sort=title&order=desc`);
	const shown = ['Signal Remix', 'Signal Part 2', 'Signal Part 1'];
	await expect(shownTitles(page)).toHaveText(shown);
	await playButton(page).click();
	await expect(player(page).getByText('Signal Remix')).toBeVisible();
	await expect.poll(() => sent.starts.length).toBe(1);
	expect(sent.starts[0]).toMatchObject({
		context: { type: 'playlist', id: mix },
		source: { recipe: { type: 'playlist', playlistId: mix, sort: 'name', order: 'desc' } }
	});
	expect(sent.starts[0].source).not.toHaveProperty('tracks');
	await expect.poll(async () => (await lists(server))[0]).toEqual(ids(server, ...shown));

	await main(page).getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'playlist', sort: 'name', order: 'desc' } }
		});
	await expect.poll(async () => (await lists(server)).at(-1)).toEqual(ids(server, ...shown));
});

test('a sorted, tag-filtered playlist starts by a recipe resolving to the rows shown', async ({
	page,
	server
}) => {
	const { Calm } = server.ids.tags;
	const sent = observe(page);
	const mix = await createPlaylist(server, 'Signal Mix', [
		'Signal Part 1',
		'Signal Part 2',
		'Signal Remix'
	]);
	// The start is not answered yet, so Up Next is what the device made from the rows shown.
	const answer = await hold(page, '**/api/v1/me/queue', isStart);
	// Added together, so by track ID, newest first the other way round; Calm leaves Part 2 out.
	const shown = byId(server, ['Signal Part 1', 'Signal Remix']).toReversed();
	await page.goto(`/playlists/${mix}?sort=added&order=desc&tagAny=${Calm}`);
	await expect(shownTitles(page)).toHaveText(shown);
	await playButton(page).click();
	// The two rows shown, so one is up next and none is behind, as the start's total says.
	await player(page).getByRole('button', { name: 'Queue' }).click();
	const tabs = panelOf(page).getByRole('tab');
	await expect(tabs).toHaveText(['Up Next 1', 'History 0', 'Kept']);
	answer();
	await expect.poll(() => sent.starts.length).toBe(1);
	expect(sent.starts[0]).toMatchObject({
		source: {
			recipe: {
				type: 'playlist',
				playlistId: mix,
				filters: { tagAny: [Calm] },
				sort: 'addedToPlaylist',
				order: 'desc'
			}
		},
		context: { type: 'playlist', id: mix }
	});
	await expect.poll(async () => (await lists(server))[0]).toEqual(ids(server, ...shown));
});

test('an unsorted, unfiltered playlist starts by a recipe of its own order', async ({
	page,
	server
}) => {
	const sent = observe(page);
	const titles = ['Signal Part 1', 'Signal Part 2', 'Signal Remix'];
	const mix = await createPlaylist(server, 'Signal Mix', titles);
	await page.goto(`/playlists/${mix}`);
	await expect(shownTitles(page)).toHaveCount(3);
	await playButton(page).click();
	await expect.poll(() => sent.starts.length).toBe(1);
	expect(sent.starts[0].source.recipe).toEqual({ type: 'playlist', playlistId: mix });
	expect(sent.starts[0]).toMatchObject({ start: 0, startTrackId: server.ids.tracks[titles[0]] });
	await expect.poll(async () => (await lists(server))[0]).toEqual(ids(server, ...titles));
});

test('a filtered playlist sends its songs until a tag type is known, then a recipe', async ({
	page,
	server
}) => {
	const { Ambient, Folk } = server.ids.tags;
	const sent = observe(page);
	const mix = await createPlaylist(server, 'Signal Mix', [
		'Signal Part 1',
		'Signal Part 2',
		'Signal Remix'
	]);
	await page.goto(`/playlists/${mix}`);
	await playButton(page).click();
	await expect.poll(() => sent.starts.length).toBe(1);

	// Folk is a genre no song of the playlist carries, and its type is not read yet.
	const release = await hold(page, `**/tags/${Folk}`);
	await page.goto(`/playlists/${mix}?tagAny=${Ambient},${Folk}`);
	await queueLoaded(page);
	await main(page).getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	// Nothing is shown to send: no recipe went for rows the server might read differently.
	await expect(page.getByRole('status')).toContainText('no songs in “Signal Mix”');
	expect(sent.edits).toEqual([]);

	release();
	await expect(shownTitles(page)).toHaveText(['Signal Part 1', 'Signal Part 2']);
	await main(page).getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'playlist', filters: { tagAny: [Ambient, Folk] } } }
		});
});

test('queues a tag-filtered view by a recipe carrying its filter', async ({ page, server }) => {
	const { Calm, Ambient } = server.ids.tags;
	const { Signal } = server.ids.albums;
	const sent = observe(page);
	// The album filtered to Calm: Part 2 is left out.
	await page.goto(`/albums/${Signal}?tagAny=${Calm}`);
	const shown = ['Signal Part 1', 'Signal Remix'];
	await expect(shownTitles(page)).toHaveText(shown);
	await playButton(page).click();
	await expect(player(page).getByText('Signal Part 1')).toBeVisible();
	await expect.poll(() => sent.starts.length).toBe(1);
	expect(sent.starts.at(-1)).toMatchObject({
		source: { recipe: { type: 'album', albumId: Signal, filters: { tagAny: [Calm] } } }
	});
	await main(page).getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'album', albumId: Signal, filters: { tagAny: [Calm] } } }
		});
	expect(sent.edits.at(-1)).not.toHaveProperty('source.tracks');
	// What the server resolves for it is exactly the songs shown.
	await expect.poll(async () => (await lists(server)).at(-1)).toEqual(ids(server, ...shown));

	// All the songs filtered to Ambient, the same way.
	await page.goto(`/songs?tagAny=${Ambient}`);
	await expect(shownTitles(page)).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await queueLoaded(page);
	await main(page).getByRole('button', { name: 'Actions for all songs' }).click();
	await page.getByRole('menuitem', { name: 'Queue next' }).click();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'library', filters: { tagAny: [Ambient] } } }
		});
	await expect
		.poll(async () => (await lists(server)).at(-1))
		.toEqual(ids(server, 'Signal Part 2', 'Signal Part 1'));
});

test('a filtered album sends its songs until a tag type is known, then a recipe', async ({
	page,
	server
}) => {
	const { Ambient, Folk } = server.ids.tags;
	const { Signal } = server.ids.albums;
	const sent = observe(page);
	await page.goto(`/albums/${Signal}`);
	await playButton(page).click();
	await expect.poll(() => sent.starts.length).toBe(1);

	// Folk is a genre no song of the album carries, and its type is not read yet.
	const release = await hold(page, `**/tags/${Folk}`);
	await page.goto(`/albums/${Signal}?tagAny=${Ambient},${Folk}`);
	await queueLoaded(page);
	await main(page).getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	// Nothing is shown to send: no recipe went for rows the server might read differently.
	await expect(page.getByRole('status')).toContainText('no songs in “Signal”');
	expect(sent.edits).toEqual([]);

	release();
	await expect(shownTitles(page)).toHaveText(['Signal Part 1', 'Signal Part 2']);
	await main(page).getByRole('button', { name: 'Actions for “Signal”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'album', filters: { tagAny: [Ambient, Folk] } } }
		});
});

test('queues every song a filtered search finds', async ({ page, server }) => {
	const { Ambient } = server.ids.tags;
	const sent = observe(page);
	await page.goto(`/search?q=signal&tagAny=${Ambient}`);
	await expect(shownTitles(page)).toHaveCount(2);
	// In the order the server found them.
	const found = (await shownTitles(page).allInnerTexts()).map((t) => t.trim());
	expect(found.toSorted()).toEqual(['Signal Part 1', 'Signal Part 2']);
	await page.getByRole('button', { name: 'Signal Part 2', exact: true }).click();
	await expect(player(page).getByText('Signal Part 2')).toBeVisible();
	await expect.poll(() => sent.starts.length).toBe(1);
	expect(sent.starts.at(-1)).toMatchObject({
		source: { recipe: { type: 'search', q: 'signal', filters: { tagAny: [Ambient] } } }
	});
	await main(page).getByRole('button', { name: 'Actions for all songs found' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect
		.poll(() => sent.edits.at(-1))
		.toMatchObject({
			type: 'add',
			source: { recipe: { type: 'search', q: 'signal', filters: { tagAny: [Ambient] } } }
		});
	await expect.poll(async () => (await lists(server)).at(-1)).toEqual(ids(server, ...found));
});

test.describe('on a server of its own, with no queue', () => {
	test.use({ loggedIn: false });

	test('says to start something first when there is no queue to add to', async ({
		page,
		freshServer
	}) => {
		const { url } = await freshServer();
		const { username, password, token } = await setupOwner(url);
		const api = async (path: string) =>
			(
				await fetch(`${url}/api/v1${path}`, { headers: { Authorization: `Bearer ${token}` } })
			).json();
		const library = (await api('/libraries')).items[0].id;
		await expect.poll(async () => (await api(`/libraries/${library}`)).trackCount).toBe(1);
		const album = (await api(`/libraries/${library}/albums`)).items[0].id;

		const sent = observe(page);
		await login(page, url, username, password);
		await page.goto(`${url}/albums/${album}`);
		await main(page)
			.getByRole('button', { name: /^Actions for “/ })
			.click();
		await page.getByRole('menuitem', { name: 'Queue last' }).click();
		await expect(
			page.getByRole('status').getByText('Start something playing first, then queue more.')
		).toBeVisible();
		expect(sent.edits).toEqual([]);
	});
});

/** The position, in ms, of the last progress report whose item ID matches `item`, or 0 if none. */
const lastPosition = (sent: ReturnType<typeof observe>, item: RegExp) =>
	sent.reports.findLast((r) => item.test(r.itemId))?.positionMs ?? 0;

test('a queued-into queue is kept and restores where it was', async ({ page, server }) => {
	const sent = observe(page);
	const { player, panel } = await playAlbum(page, server);
	// Let it play a moment, so there is a place to come back to, then pause: the short audio
	// must not end and move the queue on.
	await expect.poll(() => lastPosition(sent, /\.0$/), { timeout: 15_000 }).toBeGreaterThan(0);
	await player.getByRole('button', { name: 'Pause' }).click();
	// Queue a song, so the queue was adjusted.
	await main(page).getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect.poll(() => sent.edits.at(-1)).toMatchObject({ type: 'add' });
	await expect
		.poll(() => sent.reports.at(-1))
		.toMatchObject({ status: 'paused', itemId: expect.stringMatching(/\.0$/) });
	const left = sent.reports.at(-1)!.positionMs;
	expect(left).toBeGreaterThan(0);
	const first = (await server.api('/me/queue')).queueId;

	// Starting another queue replaces it, and keeps it.
	await playButton(page).click();
	await expect.poll(() => sent.starts.length).toBe(2);
	await expect.poll(() => kept(server)).toEqual([first]);

	await player.getByRole('button', { name: 'Queue' }).click();
	await panel.getByRole('tab', { name: 'Kept' }).click();
	const entry = panel.getByRole('list', { name: 'Kept queues' }).getByRole('listitem');
	await expect(entry).toHaveCount(1);
	await expect(entry).toContainText('Signal Part 1');
	await expect(entry).toContainText('Signal');
	await expect(entry).toContainText('3 songs up next');
	await expect(entry).toContainText('0 plays played');

	sent.reports.length = 0;
	const restored = page.waitForResponse((r) => r.url().endsWith(`/me/queues/${first}/restore`));
	await entry.getByRole('button', { name: /^Restore/ }).click();
	await expect.poll(() => sent.restores).toEqual([first]);
	// The server has the place where playback was left.
	const { position } = await (await restored).json();
	expect(position).toEqual({ item: expect.stringMatching(/\.0$/), positionMs: left });
	// It is the queue shown, and the first thing reported is a start of its item, where it was left.
	await expect(panel.getByRole('tab', { name: /Up Next/ })).toHaveAttribute(
		'aria-selected',
		'true'
	);
	await expect
		.poll(() => sent.reports[0])
		.toMatchObject({ itemId: position.item, start: true, positionMs: left });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
	await expect(
		panel
			.getByRole('list', { name: 'Up Next' })
			.getByRole('button', { name: 'Signal Remix', exact: true })
	).toHaveCount(2);
	await expect.poll(() => activeQueue(server)).toBe(first);

	// The queue it replaced was never adjusted, so nothing is kept now.
	await panel.getByRole('tab', { name: 'Kept' }).click();
	await expect(panel.getByText(/kept here for 30 days/)).toBeVisible();
	expect(await kept(server)).toEqual([]);
});

test('deletes a kept queue with one press', async ({ page, server }) => {
	const sent = observe(page);
	const { player, panel } = await playAlbum(page, server);
	await main(page).getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	// A start drops the edits not yet sent, so the queue is adjusted, and kept, only once it is.
	await expect.poll(() => sent.edits.at(-1)).toMatchObject({ type: 'add' });
	await playButton(page).click();
	await expect.poll(async () => (await kept(server)).length).toBe(1);

	await player.getByRole('button', { name: 'Queue' }).click();
	await panel.getByRole('tab', { name: 'Kept' }).click();
	await panel.getByRole('button', { name: /^Delete/ }).click();
	await expect(panel.getByText(/kept here for 30 days/)).toBeVisible();
	expect(await kept(server)).toEqual([]);
});

test('restores a kept queue on a device that holds no playback, while its lists are slow', async ({
	page,
	server
}) => {
	const sent = observe(page);
	const { player, panel } = await playAlbum(page, server);
	await expect.poll(() => lastPosition(sent, /\.0$/), { timeout: 15_000 }).toBeGreaterThan(0);
	await player.getByRole('button', { name: 'Pause' }).click();
	await main(page).getByRole('button', { name: 'Actions for Signal Remix' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await expect.poll(() => sent.edits.at(-1)).toMatchObject({ type: 'add' });
	await expect
		.poll(() => sent.reports.at(-1))
		.toMatchObject({ status: 'paused', itemId: expect.stringMatching(/\.0$/) });
	const left = sent.reports.at(-1)!.positionMs;
	await playButton(page).click();
	await expect.poll(async () => (await kept(server)).length).toBe(1);

	// A fresh page holds no playback and no lists, and the server is slow to give them.
	await page.route(/\/me\/queue\/(rows|lists\/)/, async (route) => {
		await new Promise((resolve) => setTimeout(resolve, 1_500));
		await route.continue();
	});
	await page.reload();
	await player.getByRole('button', { name: 'Queue' }).click();
	await panel.getByRole('tab', { name: 'Kept' }).click();
	sent.reports.length = 0;
	await panel.getByRole('button', { name: /^Restore/ }).click();
	await expect.poll(() => sent.restores.length).toBe(1);
	await expect
		.poll(() => sent.reports[0], { timeout: 10_000 })
		.toMatchObject({ itemId: expect.stringMatching(/\.0$/), start: true, positionMs: left });
	await expect(player.getByText('Signal Part 1')).toBeVisible();
});

test('a filtered playlist queues as many pending rows as it shows', async ({ page, server }) => {
	const { Calm } = server.ids.tags;
	const mix = await createPlaylist(server, 'Signal Mix', [
		'Signal Part 1',
		'Signal Part 2',
		'Signal Remix'
	]);
	const sent = observe(page);
	await page.goto(`/albums/${server.ids.albums.Signal}`);
	await playButton(page).click();
	await expect.poll(() => sent.starts.length).toBe(1);
	// The server does not answer the add yet.
	const release = await hold(page, '**/api/v1/me/queue/edits');
	await page.goto(`/playlists/${mix}?tagAny=${Calm}`);
	await expect(shownTitles(page)).toHaveText(['Signal Part 1', 'Signal Remix']);
	await queueLoaded(page);
	await main(page).getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	await player(page).getByRole('button', { name: 'Queue' }).click();
	// Two rows, not the playlist's three.
	await expect(panelOf(page).getByRole('heading', { name: /Queued by you\s*2/ })).toBeVisible();
	release();
});

test('a start from a clicked row of a playlist names the row and its track', async ({
	page,
	server
}) => {
	const sent = observe(page);
	const album = ['Signal Part 1', 'Signal Part 2', 'Signal Remix'];
	const mix = await createPlaylist(
		server,
		'Long Mix',
		Array.from({ length: 15 }, () => album).flat()
	);
	await page.goto(`/playlists/${mix}`);
	// The second Signal Part 2 is row 4; the first is row 1.
	await page.getByRole('button', { name: 'Signal Part 2', exact: true }).nth(1).dblclick();
	await expect.poll(() => sent.starts.length).toBe(1);
	expect(sent.starts[0]).toMatchObject({
		source: { recipe: { type: 'playlist', playlistId: mix } },
		start: 4,
		startTrackId: server.ids.tracks['Signal Part 2']
	});
	// The server starts at that row, not the track's first place.
	await expect.poll(async () => (await server.api('/me/queue')).state.current).toMatch(/\.4$/);
});

test('a playlist with edits not yet saved starts from the rows shown, then by recipe', async ({
	page,
	server
}) => {
	const sent = observe(page);
	const mix = await createPlaylist(server, 'Signal Mix', [
		'Signal Part 1',
		'Signal Part 2',
		'Signal Remix'
	]);
	await page.goto(`/playlists/${mix}`);
	const release = await hold(page, `**/playlists/${mix}`, (r) => r.method() === 'PATCH');
	await page.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await page
		.getByRole('toolbar', { name: 'Selected songs' })
		.getByRole('button', { name: 'Remove' })
		.click();
	await expect(shownTitles(page)).toHaveText(['Signal Part 1', 'Signal Part 2']);
	const play = playButton(page);
	await play.click();
	// The server still has the removed song, so the songs shown go.
	await expect.poll(() => sent.starts.length).toBe(1);
	expect(sent.starts[0].source).toMatchObject({
		tracks: ids(server, 'Signal Part 1', 'Signal Part 2')
	});
	expect(sent.starts[0].source).not.toHaveProperty('recipe');

	const saved = page.waitForResponse((r) => r.request().method() === 'PATCH');
	release();
	await saved;
	await expect(async () => {
		await play.click();
		expect(sent.starts.at(-1)!.source).toHaveProperty('recipe.type', 'playlist');
	}).toPass();
});

test('a sorted playlist queued while an edit saves goes in the order shown', async ({
	page,
	server
}) => {
	const sent = observe(page);
	const mix = await createPlaylist(server, 'Signal Mix', [
		'Signal Part 1',
		'Signal Part 2',
		'Signal Remix'
	]);
	await page.goto(`/playlists/${mix}?sort=title&order=desc`);
	await playButton(page).click();
	await expect.poll(() => sent.starts.length).toBe(1);
	const release = await hold(page, `**/playlists/${mix}`, (r) => r.method() === 'PATCH');
	await page.getByRole('button', { name: 'Signal Remix', exact: true }).click();
	await page
		.getByRole('toolbar', { name: 'Selected songs' })
		.getByRole('button', { name: 'Remove' })
		.click();
	await expect(shownTitles(page)).toHaveText(['Signal Part 2', 'Signal Part 1']);
	await main(page).getByRole('button', { name: 'Actions for “Signal Mix”' }).click();
	await page.getByRole('menuitem', { name: 'Queue last' }).click();
	// Not yet saved, so the songs go as shown, not in the playlist's own order.
	await expect
		.poll(() => sent.edits.findLast((edit) => edit.type === 'add'))
		.toMatchObject({ source: { tracks: ids(server, 'Signal Part 2', 'Signal Part 1') } });
	release();
});

test('a playlist sorted by date added plays the rows shown once songs were put back', async ({
	page,
	server
}) => {
	const sent = observe(page);
	const titles = ['Signal Part 1', 'Signal Part 2', 'Signal Remix'];
	const mix = await createPlaylist(server, 'Signal Mix', titles);
	await page.goto(`/playlists/${mix}?sort=added&order=asc`);
	// Added together, so by track ID.
	const shown = byId(server, titles);
	await expect(shownTitles(page)).toHaveText(shown);
	const saved = () => page.waitForResponse((r) => r.request().method() === 'PATCH');
	// The first row, so that its coming back dated now would move it.
	await page.getByRole('button', { name: shown[0], exact: true }).click();
	let save = saved();
	await page
		.getByRole('toolbar', { name: 'Selected songs' })
		.getByRole('button', { name: 'Remove' })
		.click();
	await save;
	save = saved();
	await page.getByRole('button', { name: 'Undo' }).click();
	await save;
	// The server dated the song put back as added now, the page as it was, so the rows shown go.
	await expect(shownTitles(page)).toHaveText(shown);
	await playButton(page).click();
	await expect.poll(() => sent.starts.length).toBe(1);
	expect(sent.starts[0].source).toMatchObject({ tracks: ids(server, ...shown) });
	expect(sent.starts[0].source).not.toHaveProperty('recipe');
});
