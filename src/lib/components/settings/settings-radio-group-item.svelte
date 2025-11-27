<script lang="ts">
  import { Label } from "$lib/components/ui/label";
  import * as RadioGroup from "$lib/components/ui/radio-group";
  import { cn } from "$lib/utils.js";

  type Props = {
    value: string;
    class?: string;
    disabled?: boolean;
    children?: import('svelte').Snippet;
  };
 
  let {
    value,
    class: className,
    disabled = false,
    children
  }: Props = $props();

</script>

<Label
  for="radio-{value}"
  class={cn(
    "group flex items-center gap-3 rounded-lg border p-4 transition-colors",
    disabled ? "cursor-default" : "cursor-pointer hover:bg-muted/50",
    className
  )}
  aria-disabled={disabled}
>
  <RadioGroup.Item {value} id="radio-{value}" {disabled} />
  {#if children}
      {@render children()}
  {:else}
      {value}
  {/if}
</Label>
