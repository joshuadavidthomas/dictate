<script lang="ts">
  import * as RadioGroup from "$lib/components/ui/radio-group";
  import SettingsLabel from "./settings-label.svelte";
  import { cn } from "$lib/utils.js";
  
  type Props = {
    id: string;
    label: string;
    value?: string;
    onValueChange?: (value: string) => void;
    class?: string;
    description?: import('svelte').Snippet;
    children: import('svelte').Snippet;
  };
  
  let {
    id,
    label: labelText,
    value = $bindable(""),
    onValueChange,
    class: className,
    description,
    children
  }: Props = $props();
</script>

<div class={cn("space-y-4", className)}>
  <SettingsLabel for={id} label={labelText} {description} />
  <RadioGroup.Root id={id} bind:value {onValueChange}>
    <div class="flex flex-col gap-2">
      {@render children()}
    </div>
  </RadioGroup.Root>
</div>
