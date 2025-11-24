<script lang="ts">
  import { createTranscriptionsState } from "$lib/stores";

  createTranscriptionsState();

  let { children } = $props();
</script>

{@render children()}
