<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import * as Kbd from "$lib/components/ui/kbd";
  import SettingsRow from "./settings-row.svelte";
  import SettingsLabel from "./settings-label.svelte";
  import XIcon from "@lucide/svelte/icons/x";
  import KeyboardIcon from "@lucide/svelte/icons/keyboard";
  
  type Props = {
    id: string;
    label: string;
    value?: string | null;
    onChange?: (value: string | null) => Promise<void>;
    class?: string;
    description?: import('svelte').Snippet;
  };
  
  let {
    id,
    label: labelText,
    value = $bindable(null),
    onChange,
    class: className,
    description
  }: Props = $props();
  
  let isRecording = $state(false);
  let tempKeys = $state<string[]>([]);
  let error = $state<string | null>(null);
  
  const modifierMap: Record<string, string> = {
    'Control': 'Ctrl',
    'Meta': 'Super',
    'Alt': 'Alt',
    'Shift': 'Shift'
  };
  
  function getShortcutKeys(shortcut: string | null): string[] {
    if (!shortcut) return [];
    return shortcut
      .replace('CommandOrControl', 'Ctrl')
      .replace('Command', 'Super')
      .split('+');
  }
  
  function handleKeyDown(event: KeyboardEvent) {
    if (!isRecording) return;
    
    event.preventDefault();
    event.stopPropagation();
    
    const keys: string[] = [];
    
    // Add modifiers in consistent order
    if (event.ctrlKey || event.metaKey) keys.push(event.ctrlKey ? 'Ctrl' : 'Super');
    if (event.altKey) keys.push('Alt');
    if (event.shiftKey) keys.push('Shift');
    
    // Add the actual key (if it's not a modifier)
    const key = event.key;
    if (!['Control', 'Meta', 'Alt', 'Shift'].includes(key)) {
      // Normalize key name
      let normalizedKey = key;
      if (key === ' ') normalizedKey = 'Space';
      else if (key.length === 1) normalizedKey = key.toUpperCase();
      
      keys.push(normalizedKey);
      
      // We have a complete shortcut (modifier + key)
      if (keys.length > 1) {
        tempKeys = keys;
      }
    } else {
      tempKeys = keys;
    }
  }
  
  function handleKeyUp(event: KeyboardEvent) {
    if (!isRecording) return;
    
    event.preventDefault();
    event.stopPropagation();
    
    // Only commit when we have at least one modifier and one regular key
    if (tempKeys.length > 1) {
      commitShortcut();
    }
  }
  
  async function commitShortcut() {
    if (tempKeys.length === 0) {
      isRecording = false;
      return;
    }
    
    // Convert to backend format (CommandOrControl+Shift+Space)
    const backendFormat = tempKeys
      .map(key => {
        if (key === 'Ctrl' || key === 'Super') return 'CommandOrControl';
        return key;
      })
      .join('+');
    
    try {
      error = null;
      if (onChange) {
        await onChange(backendFormat);
      }
      value = backendFormat;
      isRecording = false;
      tempKeys = [];
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to set shortcut';
      tempKeys = [];
    }
  }
  
  function startRecording() {
    isRecording = true;
    tempKeys = [];
    error = null;
  }
  
  function cancelRecording() {
    isRecording = false;
    tempKeys = [];
    error = null;
  }
  
  async function clearShortcut() {
    try {
      error = null;
      if (onChange) {
        await onChange(null);
      }
      value = null;
    } catch (err) {
      error = err instanceof Error ? err.message : 'Failed to clear shortcut';
    }
  }
  
  $effect(() => {
    if (isRecording) {
      window.addEventListener('keydown', handleKeyDown);
      window.addEventListener('keyup', handleKeyUp);
      
      return () => {
        window.removeEventListener('keydown', handleKeyDown);
        window.removeEventListener('keyup', handleKeyUp);
      };
    }
  });
</script>

<SettingsRow class={className}>
  {#snippet label()}
    <SettingsLabel for={id} label={labelText} {description} />
  {/snippet}
  {#snippet control()}
    <div class="flex flex-col gap-2 items-end">
      <div class="flex gap-2 items-center">
        <button
          type="button"
          onclick={startRecording}
          class="min-w-[200px] h-9 px-3 rounded-md border bg-background cursor-pointer flex items-center justify-center gap-1 {isRecording ? 'ring-2 ring-primary ring-offset-2' : 'hover:bg-accent'}"
        >
          {#if isRecording}
            {#if tempKeys.length > 0}
              <Kbd.Group>
                {#each tempKeys as key, i}
                  {#if i > 0}<span class="text-muted-foreground">+</span>{/if}
                  <Kbd.Root>{key}</Kbd.Root>
                {/each}
              </Kbd.Group>
            {:else}
              <span class="text-muted-foreground text-sm flex items-center gap-2">
                <KeyboardIcon class="w-4 h-4 animate-pulse" />
                Press keys...
              </span>
            {/if}
          {:else if value}
            <Kbd.Group>
              {#each getShortcutKeys(value) as key, i}
                {#if i > 0}<span class="text-muted-foreground">+</span>{/if}
                <Kbd.Root>{key}</Kbd.Root>
              {/each}
            </Kbd.Group>
          {:else}
            <span class="text-muted-foreground text-sm">Click to set shortcut</span>
          {/if}
        </button>
        
        {#if isRecording}
          <Button
            size="sm"
            variant="outline"
            onclick={cancelRecording}
          >
            Cancel
          </Button>
        {:else if value}
          <Button
            size="sm"
            variant="ghost"
            onclick={clearShortcut}
          >
            <XIcon class="w-4 h-4" />
          </Button>
        {/if}
      </div>
      
      {#if error}
        <p class="text-sm text-destructive">{error}</p>
      {/if}
    </div>
  {/snippet}
</SettingsRow>
