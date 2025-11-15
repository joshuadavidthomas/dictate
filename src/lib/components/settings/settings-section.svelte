<script lang="ts">
  import { cn } from "$lib/utils.js";
  
  type Props = {
    class?: string;
    children: import('svelte').Snippet;
  };
  
  let { class: className, children }: Props = $props();
</script>

<div class={cn("space-y-8", className)}>
  {@render children()}
</div>
