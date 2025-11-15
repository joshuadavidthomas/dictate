<script lang="ts">
  import { cn } from "$lib/utils.js";
  
  type Props = {
    class?: string;
    children?: import('svelte').Snippet;
    label?: import('svelte').Snippet;
    control?: import('svelte').Snippet;
  };
  
  let { class: className, children, label, control }: Props = $props();
</script>

<div class={cn("flex flex-row gap-4 items-center justify-between", className)}>
  {#if children}
    {@render children()}
  {:else}
    {#if label}
      <div class="space-y-1 flex-1">
        {@render label()}
      </div>
    {/if}
    {#if control}
      {@render control()}
    {/if}
  {/if}
</div>
