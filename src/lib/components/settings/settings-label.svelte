<script lang="ts">
  import { Label } from "$lib/components/ui/label";
  import { cn } from "$lib/utils.js";
  
  type Props = {
    for?: string;
    label: string;
    class?: string;
    labelClass?: string;
    descriptionClass?: string;
    description?: import('svelte').Snippet;
  };
  
  let {
    for: htmlFor,
    label,
    class: className,
    labelClass,
    descriptionClass,
    description
  }: Props = $props();
</script>

<div class={cn("space-y-1", className)}>
  <Label for={htmlFor} class={cn("font-medium", labelClass)}>
    {label}
  </Label>
  {#if description}
    <p class={cn("text-sm text-muted-foreground", descriptionClass)}>
      {@render description()}
    </p>
  {/if}
</div>
