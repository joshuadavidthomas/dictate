<script lang="ts">
  import * as RadioGroup from "$lib/components/ui/radio-group";
  import SettingsLabel from "./settings-label.svelte";
  import { cn } from "$lib/utils.js";
  import { setContext } from "svelte";
  
  type Props = {
    id: string;
    label: string;
    value?: string;
    onValueChange?: (value: string) => void;
    class?: string;
    gridClass?: string;
    description?: import('svelte').Snippet;
    children: import('svelte').Snippet;
  };
  
  let {
    id,
    label: labelText,
    value = $bindable(""),
    onValueChange,
    class: className,
    gridClass,
    description,
    children
  }: Props = $props();
  
  const RADIO_CARDS_VALUE_KEY = "RADIO_CARDS_VALUE";
  setContext(RADIO_CARDS_VALUE_KEY, { getValue: () => value });
</script>

<div class={cn("space-y-4", className)}>
  <SettingsLabel for={id} label={labelText} {description} />
  <RadioGroup.Root id={id} bind:value {onValueChange} class={cn("grid grid-cols-1 md:grid-cols-2 gap-4", gridClass)}>
    {@render children()}
  </RadioGroup.Root>
</div>
