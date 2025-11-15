<script lang="ts">
  import { Switch } from "$lib/components/ui/switch";
  import SettingsRow from "./settings-row.svelte";
  import SettingsLabel from "./settings-label.svelte";
  
  type Props = {
    id: string;
    label: string;
    checked?: boolean;
    onCheckedChange?: (checked: boolean) => void;
    class?: string;
    description?: import('svelte').Snippet;
  };
  
  let {
    id,
    label: labelText,
    checked = $bindable(false),
    onCheckedChange,
    class: className,
    description
  }: Props = $props();
</script>

<SettingsRow class={className}>
  {#snippet label()}
    <SettingsLabel for={id} label={labelText} {description} />
  {/snippet}
  {#snippet control()}
    <Switch
      id={id}
      bind:checked
      {onCheckedChange}
    />
  {/snippet}
</SettingsRow>
