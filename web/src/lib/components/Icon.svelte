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
		close: [['path', { d: 'M6 6l12 12M18 6 6 18' }]]
	} satisfies Record<string, Shape[]>;

	/** Transport icons are solid; the rest are strokes. */
	const filled = new Set(['play', 'pause', 'next', 'previous']);

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
