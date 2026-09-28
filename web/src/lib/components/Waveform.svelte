<script lang="ts">
	import { formatDuration } from '$lib/format';
	import { bars } from '$lib/waveform';

	let {
		points,
		position,
		duration,
		onseek,
		height = 40,
		label = 'Seek'
	}: {
		/** Null draws a plain bar: not analyzed yet, or nothing playing. */
		points: Uint8Array | null;
		/** Seconds. */
		position: number;
		duration: number;
		onseek: (seconds: number) => void;
		height?: number;
		label?: string;
	} = $props();

	const uid = $props.id();
	const clipId = `played-${uid}`;
	const BAR = 2;
	const GAP = 1;

	let width = $state(0);
	/** While dragging, the position under the pointer; committed on release. */
	let dragging = $state<number | null>(null);

	const count = $derived(Math.max(1, Math.floor((width + GAP) / (BAR + GAP))));
	const heights = $derived(points ? bars(points, count) : null);
	const shown = $derived(dragging ?? position);
	const progress = $derived(duration > 0 ? Math.min(1, Math.max(0, shown / duration)) : 0);
	const disabled = $derived(duration <= 0);

	function at(e: PointerEvent, el: HTMLElement): number {
		const rect = el.getBoundingClientRect();
		return (Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width)) || 0) * duration;
	}

	function down(e: PointerEvent) {
		if (disabled || e.button !== 0) return;
		const el = e.currentTarget as HTMLElement;
		el.setPointerCapture(e.pointerId);
		dragging = at(e, el);
	}

	function move(e: PointerEvent) {
		if (dragging !== null) dragging = at(e, e.currentTarget as HTMLElement);
	}

	function up(e: PointerEvent) {
		if (dragging === null) return;
		onseek(at(e, e.currentTarget as HTMLElement));
		dragging = null;
	}

	// The keyboard path never depends on seeing the shape (`requirements/playback.md` §3).
	function key(e: KeyboardEvent) {
		if (disabled) return;
		const steps: Record<string, number> = {
			ArrowLeft: -5,
			ArrowDown: -5,
			ArrowRight: 5,
			ArrowUp: 5,
			PageDown: -30,
			PageUp: 30
		};
		if (e.key in steps) onseek(Math.min(duration, Math.max(0, position + steps[e.key])));
		else if (e.key === 'Home') onseek(0);
		else if (e.key === 'End') onseek(Math.max(0, duration - 1));
		else return;
		e.preventDefault();
	}
</script>

<div
	class="wave"
	class:disabled
	style:height="{height}px"
	bind:clientWidth={width}
	role="slider"
	tabindex={disabled ? -1 : 0}
	aria-label={label}
	aria-valuemin={0}
	aria-valuemax={Math.round(duration)}
	aria-valuenow={Math.round(shown)}
	aria-valuetext="{formatDuration(shown * 1e6)} of {formatDuration(duration * 1e6)}"
	aria-disabled={disabled}
	onpointerdown={down}
	onpointermove={move}
	onpointerup={up}
	onpointercancel={() => (dragging = null)}
	onkeydown={key}
>
	{#if heights}
		{@const w = count * (BAR + GAP) - GAP}
		<svg viewBox="0 0 {w} 100" preserveAspectRatio="none" aria-hidden="true">
			<defs>
				<clipPath id={clipId}>
					<rect x="0" y="0" width={progress * w} height="100" />
				</clipPath>
			</defs>
			{#each [false, true] as played (played)}
				<g class:played clip-path={played ? `url(#${clipId})` : undefined}>
					{#each heights as h, i (i)}
						{@const bh = Math.max(4, h * 100)}
						<rect x={i * (BAR + GAP)} y={(100 - bh) / 2} width={BAR} height={bh} rx="1" />
					{/each}
				</g>
			{/each}
		</svg>
	{:else}
		<div class="plain"><div class="fill" style:width="{progress * 100}%"></div></div>
	{/if}
</div>

<style>
	.wave {
		position: relative;
		width: 100%;
		display: flex;
		align-items: center;
		cursor: pointer;
		touch-action: none;
		border-radius: 4px;
	}

	.wave.disabled {
		cursor: default;
	}

	svg {
		width: 100%;
		height: 100%;
		display: block;
	}

	g {
		fill: var(--wave-rest);
	}

	g.played {
		fill: var(--accent);
	}

	.plain {
		width: 100%;
		height: 4px;
		border-radius: 2px;
		background: var(--wave-rest);
		overflow: hidden;
	}

	.fill {
		height: 100%;
		background: var(--accent);
	}
</style>
