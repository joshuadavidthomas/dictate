<script lang="ts">
  import { Label } from "$lib/components/ui/label";
  import * as RadioGroup from "$lib/components/ui/radio-group";
  import { cn } from "$lib/utils.js";
  import { getContext } from "svelte";

  type Props = {
    value: string;
    class?: string;
    children?: import('svelte').Snippet;
    preview?: import('svelte').Snippet;
  };

  let {
    value,
    class: className,
    children,
    preview
  }: Props = $props();

  const RADIO_CARDS_VALUE_KEY = "RADIO_CARDS_VALUE";
  const ctx = getContext<{ getValue: () => string }>(RADIO_CARDS_VALUE_KEY);
  let isSelected = $derived(ctx?.getValue() === value);
</script>

<Label
  for="radio-{value}"
  class={cn(
    "group flex cursor-pointer items-start flex-col gap-3 rounded-lg border p-4 transition-colors hover:bg-muted/50",
    isSelected && "ring-2 ring-primary bg-muted/30",
    className
  )}
>
  <div class="flex items-center gap-3 cursor-pointer">
    <RadioGroup.Item {value} id="radio-{value}" />
    {#if children}
        {@render children()}
    {:else}
        {value}
    {/if}
  </div>
  {#if preview}
    <div class="w-full">
      {@render preview()}
    </div>
  {/if}
</Label>
