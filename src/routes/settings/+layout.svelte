<script lang="ts">
  import { createAudioSettingsState } from "$lib/stores/audio-settings.svelte";
  import { createSettingsState } from "$lib/stores/settings.svelte";

	let { children } = $props();

  createSettingsState();
  createAudioSettingsState();
</script>

{@render children()}
