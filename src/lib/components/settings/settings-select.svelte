<script lang="ts">
  import * as Select from "$lib/components/ui/select";
  import SettingsRow from "./settings-row.svelte";
  import SettingsLabel from "./settings-label.svelte";
  import { cn } from "$lib/utils.js";
  
  type Props = {
    id: string;
    label: string;
    value?: string;
    onValueChange?: (value: string) => void;
    triggerClass?: string;
    class?: string;
    description?: import('svelte').Snippet;
    action?: import('svelte').Snippet;
    trigger?: import('svelte').Snippet<[{ value: string }]>;
    children: import('svelte').Snippet;
  };
  
  let {
    id,
    label: labelText,
    value = $bindable(""),
    onValueChange,
    triggerClass,
    class: className,
    description,
    action,
    trigger,
    children
  }: Props = $props();
</script>

<SettingsRow class={className}>
  {#snippet label()}
    <SettingsLabel for={id} label={labelText} {description} />
  {/snippet}
  {#snippet control()}
    <div class="flex gap-2 items-center">
      {#if action}
        {@render action()}
      {/if}
      <Select.Root type="single" bind:value {onValueChange}>
        <Select.Trigger id={id} class={cn("w-[280px]", triggerClass)}>
          {#if trigger}
            {@render trigger({ value })}
          {:else}
            {value || "Select an option"}
          {/if}
        </Select.Trigger>
        <Select.Content>
          {@render children()}
        </Select.Content>
      </Select.Root>
    </div>
  {/snippet}
</SettingsRow>
