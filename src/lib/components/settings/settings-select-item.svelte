<script lang="ts">
  import { Select as SelectPrimitive } from "bits-ui";
  import * as Select from "$lib/components/ui/select";
  import type { WithoutChild } from "$lib/utils.js";
  
  let {
    ref = $bindable(null),
    class: className,
    value,
    label,
    children,
    ...restProps
  }: WithoutChild<SelectPrimitive.ItemProps> = $props();
</script>

<Select.Item
  bind:ref
  {value}
  {label}
  class={className}
  {...restProps}
>
  {#snippet children(props)}
    {@render children?.(props)}
  {/snippet}
</Select.Item>
