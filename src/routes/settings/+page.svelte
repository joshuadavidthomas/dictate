<script lang="ts">
  import * as Alert from "$lib/components/ui/alert";
  import { Button } from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
  import { Label } from "$lib/components/ui/label";
  import * as RadioGroup from "$lib/components/ui/radio-group";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  let outputMode = $state("print");
  let showConfigChangedBanner = $state(false);
  let checkingConfig = false;

  onMount(async () => {
    // Fetch initial output mode
    const mode = await invoke("get_output_mode") as string;
    outputMode = mode;

    // Check for external changes on window focus (debounced)
    const handleFocus = async () => {
      if (checkingConfig) return; // Prevent multiple simultaneous checks
      checkingConfig = true;

      try {
        const changed = await invoke("check_config_changed") as boolean;
        if (changed) {
          // Check if file settings differ from current UI settings
          const fileMode = await invoke("get_output_mode") as string;
          if (fileMode !== outputMode) {
            showConfigChangedBanner = true;
          } else {
            // Settings match, just update mtime
            await invoke("update_config_mtime");
          }
        }
      } catch (err) {
        console.error("Failed to check config:", err);
      } finally {
        // Reset after a delay to allow for multiple focus events to settle
        setTimeout(() => {
          checkingConfig = false;
        }, 1000);
      }
    };

    window.addEventListener('focus', handleFocus);

    return () => {
      window.removeEventListener('focus', handleFocus);
    };
  });

  async function handleOutputModeChange() {
    try {
      await invoke("set_output_mode", { mode: outputMode });
      // Hide banner after successful save since we're now in sync
      showConfigChangedBanner = false;
    } catch (err) {
      console.error("Failed to set output mode:", err);
    }
  }

  async function reloadFromFile() {
    try {
      const newMode = await invoke("get_output_mode") as string;
      outputMode = newMode;
      await invoke("update_config_mtime");
      showConfigChangedBanner = false;
    } catch (err) {
      console.error("Failed to reload config:", err);
    }
  }

  async function dismissBanner() {
    try {
      // Save current UI values to file (overwrite external changes)
      await invoke("set_output_mode", { mode: outputMode });
      showConfigChangedBanner = false;
    } catch (err) {
      console.error("Failed to save config:", err);
    }
  }
</script>

<div class="flex flex-1 flex-col gap-6 p-8">
  <div class="mx-auto w-full max-w-3xl space-y-6">
    <div>
      <h1 class="text-3xl font-bold mb-2">Settings</h1>
      <p class="text-muted-foreground">Configure your transcription preferences</p>
    </div>

    <!-- Config Changed Banner -->
    {#if showConfigChangedBanner}
      <Alert.Root class="border-yellow-500 bg-yellow-50 text-yellow-900 dark:bg-yellow-950 dark:text-yellow-100">
        <AlertTriangleIcon />
        <Alert.Title>Settings file was modified externally</Alert.Title>
        <Alert.Description class="text-yellow-900 dark:text-yellow-100">
          <p>
            <strong>Reload</strong> to use external changes, or <strong>Keep Mine</strong> to save your current settings.
          </p>
          <div class="mt-4 ml-auto flex gap-2">
            <Button size="sm" onclick={reloadFromFile}>
              Reload
            </Button>
            <Button size="sm" variant="destructive" onclick={dismissBanner}>
              Keep Mine
            </Button>
          </div>
        </Alert.Description>
      </Alert.Root>
    {/if}
    <!-- Output Mode Section -->
    <Card.Root>
      <Card.Header>
        <Card.Title>Output Mode</Card.Title>
        <Card.Description>Choose how transcribed text should be handled after recording</Card.Description>
      </Card.Header>
      <Card.Content class="space-y-4">
        <RadioGroup.Root bind:value={outputMode} onValueChange={handleOutputModeChange}>
          <div class="flex items-start space-x-3 space-y-0">
            <RadioGroup.Item value="print" id="print" class="mt-1" />
            <div class="space-y-1">
              <Label for="print" class="font-medium">Print to console</Label>
              <p class="text-sm text-muted-foreground">Display transcription in the terminal output</p>
            </div>
          </div>

          <div class="flex items-start space-x-3 space-y-0">
            <RadioGroup.Item value="copy" id="copy" class="mt-1" />
            <div class="space-y-1">
              <Label for="copy" class="font-medium">Copy to clipboard</Label>
              <p class="text-sm text-muted-foreground">
                Copy transcription to system clipboard
              </p>
            </div>
          </div>

          <div class="flex items-start space-x-3 space-y-0">
            <RadioGroup.Item value="insert" id="insert" class="mt-1" />
            <div class="space-y-1">
              <Label for="insert" class="font-medium">Insert at cursor</Label>
              <p class="text-sm text-muted-foreground">
                Automatically type transcription at current cursor position
              </p>
            </div>
          </div>
        </RadioGroup.Root>

        {#if outputMode === "copy"}
          <div class="rounded-lg border bg-muted/50 p-4">
            <p class="text-sm font-medium mb-2">Required Dependencies</p>
            <p class="text-sm text-muted-foreground">
              <code class="text-xs bg-background px-2 py-1 rounded font-mono">wl-copy</code> (Wayland) or
              <code class="text-xs bg-background px-2 py-1 rounded font-mono">xclip</code> (X11)
            </p>
          </div>
        {:else if outputMode === "insert"}
          <div class="rounded-lg border bg-muted/50 p-4">
            <p class="text-sm font-medium mb-2">Required Dependencies</p>
            <p class="text-sm text-muted-foreground">
              <code class="text-xs bg-background px-2 py-1 rounded font-mono">wtype</code> (Wayland) or
              <code class="text-xs bg-background px-2 py-1 rounded font-mono">xdotool</code> (X11)
            </p>
          </div>
        {/if}
      </Card.Content>
    </Card.Root>

    <!-- Audio Settings Section (Placeholder) -->
    <Card.Root>
      <Card.Header>
        <Card.Title>Audio Settings</Card.Title>
        <Card.Description>Configure audio input and recording preferences</Card.Description>
      </Card.Header>
      <Card.Content>
        <p class="text-sm text-muted-foreground">Audio device selection coming soon...</p>
      </Card.Content>
    </Card.Root>

    <!-- Model Settings Section (Placeholder) -->
    <Card.Root>
      <Card.Header>
        <Card.Title>Transcription Model</Card.Title>
        <Card.Description>Select and manage transcription models</Card.Description>
      </Card.Header>
      <Card.Content>
        <p class="text-sm text-muted-foreground">Model selection and management coming soon...</p>
      </Card.Content>
    </Card.Root>

    <!-- Hotkey Settings Section (Placeholder) -->
    <Card.Root>
      <Card.Header>
        <Card.Title>Keyboard Shortcuts</Card.Title>
        <Card.Description>Configure global hotkeys for recording</Card.Description>
      </Card.Header>
      <Card.Content>
        <p class="text-sm text-muted-foreground">Hotkey configuration coming soon...</p>
      </Card.Content>
    </Card.Root>
  </div>
</div>
