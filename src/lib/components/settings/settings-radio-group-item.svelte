<script lang="ts">
  import { Label } from "$lib/components/ui/label";
  import * as RadioGroup from "$lib/components/ui/radio-group";
  import { cn } from "$lib/utils.js";
  
  type Props = {
    value: string;
    class?: string;
    children?: import('svelte').Snippet;
  };
  
  let {
    value,
    class: className,
    children
  }: Props = $props();
</script>

<Label
  for="radio-{value}"
  class={cn(
    "group flex cursor-pointer items-center gap-3 rounded-lg border p-4 transition-colors hover:bg-muted/50",
    className
  )}
>
  <RadioGroup.Item {value} id="radio-{value}" />
  {#if children}
    <span class="font-medium cursor-pointer">
      {@render children()}
    </span>
  {:else}
    <span class="font-medium cursor-pointer">{value}</span>
  {/if}
</Label>
