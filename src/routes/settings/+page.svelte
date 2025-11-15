<script lang="ts">
  import Heading from "$lib/components/heading.svelte";
  import OsdPreview from "$lib/components/osd-preview.svelte";
  import Page from "$lib/components/page.svelte";
  import { AudioSettings } from "$lib/components/settings";
  import * as Alert from "$lib/components/ui/alert";
  import { Button } from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
  import { Label } from "$lib/components/ui/label";
  import * as RadioGroup from "$lib/components/ui/radio-group";
  import * as Select from "$lib/components/ui/select";
  import { Switch } from "$lib/components/ui/switch";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import InfoIcon from "@lucide/svelte/icons/info";
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  let outputMode = $state("print");
  let windowDecorations = $state(true);
  let osdPosition = $state("top");
  let showConfigChangedBanner = $state(false);
  let checkingConfig = false;

  type OutputModeOption = {
    value: string;
    label: string;
    description: string;
  };

  const outputModeOptions: OutputModeOption[] = [
    {
      value: "print",
      label: "Print to console",
      description: "Display transcription in the terminal output"
    },
    {
      value: "copy",
      label: "Copy to clipboard",
      description: "Copy transcription to system clipboard"
    },
    {
      value: "insert",
      label: "Insert at cursor",
      description: "Automatically type transcription at current cursor position"
    }
  ];

  function getOutputModeLabel(mode: string): string {
    return outputModeOptions.find(opt => opt.value === mode)?.label ?? "";
  }

  onMount(() => {
    // Fetch initial settings
    (async () => {
      const mode = await invoke("get_output_mode") as string;
      outputMode = mode;

      const decorations = await invoke("get_window_decorations") as boolean;
      windowDecorations = decorations;

      const position = await invoke("get_osd_position") as string;
      osdPosition = position;
    })();

    // Check for external changes on window focus (debounced)
    const handleFocus = async () => {
      if (checkingConfig) return;
      checkingConfig = true;

      try {
        const changed = await invoke("check_config_changed") as boolean;
        if (changed) {
          const fileMode = await invoke("get_output_mode") as string;
          const fileDecorations = await invoke("get_window_decorations") as boolean;
          const filePosition = await invoke("get_osd_position") as string;
          if (fileMode !== outputMode || fileDecorations !== windowDecorations || filePosition !== osdPosition) {
            showConfigChangedBanner = true;
          } else {
            await invoke("update_config_mtime");
          }
        }
      } catch (err) {
        console.error("Failed to check config:", err);
      } finally {
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
      showConfigChangedBanner = false;
    } catch (err) {
      console.error("Failed to set output mode:", err);
    }
  }

  async function handleWindowDecorationsChange() {
    try {
      await invoke("set_window_decorations", { enabled: windowDecorations });
      showConfigChangedBanner = false;
    } catch (err) {
      console.error("Failed to set window decorations:", err);
    }
  }

  async function handleOsdPositionChange() {
    try {
      await invoke("set_osd_position", { position: osdPosition });
      showConfigChangedBanner = false;
    } catch (err) {
      console.error("Failed to set OSD position:", err);
    }
  }

  async function reloadFromFile() {
    try {
      const newMode = await invoke("get_output_mode") as string;
      outputMode = newMode;
      const newDecorations = await invoke("get_window_decorations") as boolean;
      windowDecorations = newDecorations;
      const newPosition = await invoke("get_osd_position") as string;
      osdPosition = newPosition;
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
      await invoke("set_window_decorations", { enabled: windowDecorations });
      await invoke("set_osd_position", { position: osdPosition });
      showConfigChangedBanner = false;
    } catch (err) {
      console.error("Failed to save config:", err);
    }
  }

</script>

<Page class="mx-auto max-w-6xl">
  <div>
    <Heading>Settings</Heading>
    <p class="text-muted-foreground">Configure your transcription preferences</p>
  </div>

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

  <Card.Root>
    <Card.Header>
      <Card.Title>Output Mode</Card.Title>
      <Card.Description>Choose how transcribed text should be handled after recording</Card.Description>
    </Card.Header>
    <Card.Content class="space-y-4">
      <div class="flex flex-row items-center justify-between">
        <Label for="output-mode">Mode</Label>
        <Select.Root
          type="single"
          bind:value={outputMode}
          onValueChange={handleOutputModeChange}
        >
          <Select.Trigger id="output-mode" class="w-[280px]">
            {getOutputModeLabel(outputMode) || "Select output mode"}
          </Select.Trigger>
          <Select.Content>
            {#each outputModeOptions as option}
              <Select.Item value={option.value} label={option.label}>
                <div class="flex flex-col gap-1">
                  <span class="font-medium">{option.label}</span>
                  <span class="text-xs text-muted-foreground">{option.description}</span>
                </div>
              </Select.Item>
            {/each}
          </Select.Content>
        </Select.Root>
      </div>
    </Card.Content>
  </Card.Root>

  <!-- Audio Settings Section -->
  <AudioSettings />

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

  <!-- Window Appearance Section -->
  <Card.Root>
    <Card.Header>
      <Card.Title>Window Appearance</Card.Title>
      <Card.Description>Customize the application window appearance</Card.Description>
    </Card.Header>
    <Card.Content class="space-y-4">
      <div class="flex items-center justify-between">
        <div class="space-y-1 flex-1">
          <Label for="window-decorations" class="font-medium">Show window titlebar</Label>
          <p class="text-sm text-muted-foreground">
            Display native window titlebar with minimize, maximize, and close buttons.
            Disable this for tiling window managers like Hyprland, i3, or sway.
          </p>
        </div>
        <Switch
          id="window-decorations"
          bind:checked={windowDecorations}
          onCheckedChange={handleWindowDecorationsChange}
        />
      </div>

      <Alert.Root>
        <InfoIcon class="h-4 w-4" />
        <Alert.Title>Note</Alert.Title>
        <Alert.Description>
          If you disable the titlebar, you can still move the window using your window manager's keyboard shortcuts.
          The setting takes effect immediately and will persist across restarts.
        </Alert.Description>
      </Alert.Root>
    </Card.Content>
  </Card.Root>

  <!-- OSD Position Section -->
  <Card.Root>
    <Card.Header>
      <Card.Title>On-Screen Display</Card.Title>
      <Card.Description>Choose where the on-screen display appears during recording</Card.Description>
    </Card.Header>
    <Card.Content class="space-y-4">
      <RadioGroup.Root bind:value={osdPosition} onValueChange={handleOsdPositionChange}>
        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
          <Label
            for="position-top"
            class={`group flex cursor-pointer items-start flex-col gap-3 rounded-lg border p-4 transition-colors hover:bg-muted/50 ${osdPosition === 'top' ? 'ring-2 ring-primary bg-muted/30' : ''}`}
          >
            <div class="flex items-center gap-3">
              <RadioGroup.Item value="top" id="position-top" />
              <span class="font-medium cursor-pointer">Top</span>
            </div>
            <OsdPreview position="top" class="w-full h-auto rounded-sm border shadow-sm transition-shadow duration-200 group-hover:shadow-md" />
          </Label>
          <Label
            for="position-bottom"
            class={`group flex cursor-pointer items-start flex-col gap-3 rounded-lg border p-4 transition-colors hover:bg-muted/50 ${osdPosition === 'bottom' ? 'ring-2 ring-primary bg-muted/30' : ''}`}
          >
            <div class="flex items-center gap-3">
              <RadioGroup.Item value="bottom" id="position-bottom" />
              <span class="font-medium cursor-pointer">Bottom</span>
            </div>
            <OsdPreview position="bottom" class="w-full h-auto rounded-sm border shadow-sm transition-shadow duration-200 group-hover:shadow-md" />
          </Label>
        </div>
      </RadioGroup.Root>
    </Card.Content>
  </Card.Root>
  <Card.Root>
    <Card.Header>
      <Card.Title>Keyboard Shortcuts</Card.Title>
      <Card.Description>Configure global hotkeys for recording</Card.Description>
    </Card.Header>
    <Card.Content>
      <p class="text-sm text-muted-foreground">Hotkey configuration coming soon...</p>
    </Card.Content>
  </Card.Root>
</Page>
