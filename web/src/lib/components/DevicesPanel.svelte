<script lang="ts">
	import { panels } from '$lib/panels.svelte';
	import { realtime } from '$lib/realtime.svelte';
	import DeviceList from './DeviceList.svelte';
	import Panel from './Panel.svelte';
</script>

{#if panels.devices}
	<Panel
		title="Devices"
		subtitle={realtime.offline ? 'Offline, so this may be out of date' : undefined}
		onclose={() => (panels.devices = false)}
	>
		<DeviceList />
	</Panel>
{/if}
