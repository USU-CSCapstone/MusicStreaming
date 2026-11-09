<script lang="ts" module>
	type Shape = [tag: string, attrs: Record<string, string | number>];

	const icons = {
		play: [
			[
				'path',
				{ d: 'M7 4.5v15a1 1 0 0 0 1.5.86l12.5-7.5a1 1 0 0 0 0-1.72L8.5 3.64A1 1 0 0 0 7 4.5Z' }
			]
		],
		pause: [
			['rect', { x: 6, y: 4, width: 4, height: 16, rx: 1 }],
			['rect', { x: 14, y: 4, width: 4, height: 16, rx: 1 }]
		],
		next: [
			['path', { d: 'M5 5.5v13a1 1 0 0 0 1.6.8l8.7-6.5a1 1 0 0 0 0-1.6L6.6 4.7A1 1 0 0 0 5 5.5Z' }],
			['rect', { x: 17, y: 4, width: 2.5, height: 16, rx: 1 }]
		],
		previous: [
			[
				'path',
				{ d: 'M19 5.5v13a1 1 0 0 1-1.6.8l-8.7-6.5a1 1 0 0 1 0-1.6l8.7-6.5A1 1 0 0 1 19 5.5Z' }
			],
			['rect', { x: 4.5, y: 4, width: 2.5, height: 16, rx: 1 }]
		],
		shuffle: [['path', { d: 'M16 3h5v5M4 20 21 3M21 16v5h-5M15 15l6 6M4 4l5 5' }]],
		repeat: [
			[
				'path',
				{ d: 'm17 2 4 4-4 4M3 12v-2a4 4 0 0 1 4-4h14M7 22l-4-4 4-4M21 12v2a4 4 0 0 1-4 4H3' }
			]
		],
		repeatOne: [
			[
				'path',
				{ d: 'm17 2 4 4-4 4M3 12v-2a4 4 0 0 1 4-4h14M7 22l-4-4 4-4M21 12v2a4 4 0 0 1-4 4H3' }
			],
			['path', { d: 'M11 10.5 12.5 9.5v5' }]
		],
		search: [
			['circle', { cx: 11, cy: 11, r: 7 }],
			['path', { d: 'm20 20-4-4' }]
		],
		album: [
			['circle', { cx: 12, cy: 12, r: 9 }],
			['circle', { cx: 12, cy: 12, r: 2.5 }]
		],
		artist: [
			['circle', { cx: 12, cy: 8, r: 4 }],
			['path', { d: 'M4 21v-1a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v1' }]
		],
		playlist: [
			['path', { d: 'M3 6h12M3 12h9M3 18h7' }],
			['path', { d: 'M21 5v10' }],
			['circle', { cx: 18, cy: 16, r: 3 }]
		],
		song: [
			['path', { d: 'M9 18V5l11-2v13' }],
			['circle', { cx: 6, cy: 18, r: 3 }],
			['circle', { cx: 17, cy: 16, r: 3 }]
		],
		grid: [
			['rect', { x: 3.5, y: 3.5, width: 7, height: 7, rx: 1.5 }],
			['rect', { x: 13.5, y: 3.5, width: 7, height: 7, rx: 1.5 }],
			['rect', { x: 3.5, y: 13.5, width: 7, height: 7, rx: 1.5 }],
			['rect', { x: 13.5, y: 13.5, width: 7, height: 7, rx: 1.5 }]
		],
		list: [['path', { d: 'M8 6h13M8 12h13M8 18h13M3.5 6h.01M3.5 12h.01M3.5 18h.01' }]],
		volume: [
			['path', { d: 'M11 5 6 9H3v6h3l5 4V5Z' }],
			['path', { d: 'M15.5 8.5a5 5 0 0 1 0 7M18.5 5.5a9 9 0 0 1 0 13' }]
		],
		mute: [
			['path', { d: 'M11 5 6 9H3v6h3l5 4V5Z' }],
			['path', { d: 'm16 9 6 6m0-6-6 6' }]
		],
		close: [['path', { d: 'M6 6l12 12M18 6 6 18' }]],
		plus: [['path', { d: 'M12 5v14M5 12h14' }]],
		more: [
			['circle', { cx: 5, cy: 12, r: 1.75 }],
			['circle', { cx: 12, cy: 12, r: 1.75 }],
			['circle', { cx: 19, cy: 12, r: 1.75 }]
		],
		edit: [['path', { d: 'M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16v4ZM13.5 6.5l4 4' }]],
		trash: [['path', { d: 'M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3' }]],
		pin: [['path', { d: 'M9 3h6l-1 7 3.5 3.5h-11L10 10 9 3ZM12 13.5V21' }]],
		check: [['path', { d: 'M5 12.5 10 17.5 19 7' }]],
		lyrics: [
			['rect', { x: 9, y: 2.5, width: 6, height: 12, rx: 3 }],
			['path', { d: 'M5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21.5' }]
		],
		plugin: [
			[
				'path',
				{
					d: 'M9 3h4a1 1 0 0 1 1 1v1.5a1.5 1.5 0 0 0 3 0V4a1 1 0 0 1 1-1h1a2 2 0 0 1 2 2v4h-1.5a1.5 1.5 0 0 0 0 3H21v7a2 2 0 0 1-2 2h-4v-1.5a1.5 1.5 0 0 0-3 0V21H5a2 2 0 0 1-2-2v-6h1.5a1.5 1.5 0 0 0 0-3H3V5a2 2 0 0 1 2-2h4Z'
				}
			]
		],
		warning: [
			[
				'path',
				{ d: 'M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z' }
			],
			['path', { d: 'M12 9v4M12 17h.01' }]
		],
		devices: [
			['rect', { x: 2, y: 4, width: 13, height: 10, rx: 1.5 }],
			['path', { d: 'M5 18h7' }],
			['rect', { x: 17, y: 8, width: 5, height: 12, rx: 1 }]
		],
		phone: [
			['rect', { x: 7, y: 2.5, width: 10, height: 19, rx: 2 }],
			['path', { d: 'M11 18h2' }]
		],
		tablet: [
			['rect', { x: 4, y: 2.5, width: 16, height: 19, rx: 2 }],
			['path', { d: 'M11 18h2' }]
		],
		desktop: [
			['rect', { x: 3, y: 4, width: 18, height: 12, rx: 2 }],
			['path', { d: 'M8 20h8M12 16v4' }]
		],
		tv: [
			['rect', { x: 2, y: 5, width: 20, height: 13, rx: 2 }],
			['path', { d: 'M8 21h8' }]
		],
		folder: [
			['path', { d: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z' }]
		],
		upload: [
			['path', { d: 'M12 15V4m0 0L7.5 8.5M12 4l4.5 4.5M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3' }]
		],
		up: [['path', { d: 'M12 19V5M5.5 11.5 12 5l6.5 6.5' }]],
		down: [['path', { d: 'M12 5v14M5.5 12.5 12 19l6.5-6.5' }]],
		// Opens a menu or panel below, where `down` is a descending sort.
		chevron: [['path', { d: 'm6 9 6 6 6-6' }]],
		heart: [
			[
				'path',
				{
					d: 'M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z'
				}
			]
		],
		// The heart filled in, for a liked song.
		liked: [
			[
				'path',
				{
					d: 'M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z'
				}
			]
		],
		// An eye struck through, for hiding a song.
		hide: [
			['path', { d: 'M9.88 9.88a3 3 0 1 0 4.24 4.24' }],
			[
				'path',
				{ d: 'M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68' }
			],
			['path', { d: 'M6.61 6.61A13.53 13.53 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61' }],
			['path', { d: 'm2 2 20 20' }]
		],
		tag: [
			[
				'path',
				{
					d: 'M3 12.2V4a1 1 0 0 1 1-1h8.2a2 2 0 0 1 1.4.6l7.2 7.2a2 2 0 0 1 0 2.8l-7.4 7.4a2 2 0 0 1-2.8 0l-7.2-7.2a2 2 0 0 1-.4-.6Z'
				}
			],
			['circle', { cx: 8, cy: 8, r: 1.5 }]
		],
		grip: [
			['path', { d: 'M9 5h.01M15 5h.01M9 12h.01M15 12h.01M9 19h.01M15 19h.01', 'stroke-width': 3 }]
		],
		clock: [
			['circle', { cx: 12, cy: 12, r: 9 }],
			['path', { d: 'M12 7v5l3 2' }]
		]
	} satisfies Record<string, Shape[]>;

	/** Transport icons and ⋯ are solid; the rest are strokes. */
	const filled = new Set(['play', 'pause', 'next', 'previous', 'more', 'liked']);

	export type IconName = keyof typeof icons;
</script>

<script lang="ts">
	let { name, size = 20 }: { name: IconName; size?: number } = $props();
</script>

<svg
	width={size}
	height={size}
	viewBox="0 0 24 24"
	fill={filled.has(name) ? 'currentColor' : 'none'}
	stroke={filled.has(name) ? 'none' : 'currentColor'}
	stroke-width="2"
	stroke-linecap="round"
	stroke-linejoin="round"
	aria-hidden="true"
>
	{#each icons[name] as [tag, attrs], i (i)}
		<svelte:element this={tag} {...attrs} />
	{/each}
</svg>
