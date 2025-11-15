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
  import { settings } from "$lib/stores";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import { onMount } from "svelte";

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
    // Load initial settings
    settings.load();

    // Check for external changes on window focus
    const handleFocus = () => {
      settings.checkConfigChanged();
    };

    window.addEventListener('focus', handleFocus);

    return () => {
      window.removeEventListener('focus', handleFocus);
    };
  });
</script>

<Page class="mx-auto max-w-6xl">
  <div>
    <Heading>Settings</Heading>
    <p class="text-muted-foreground">Configure your transcription preferences</p>
  </div>

  {#if settings.configChanged}
    <Alert.Root class="border-yellow-500 bg-yellow-50 text-yellow-900 dark:bg-yellow-950 dark:text-yellow-100">
      <AlertTriangleIcon />
      <Alert.Title>Settings file was modified externally</Alert.Title>
      <Alert.Description class="text-yellow-900 dark:text-yellow-100">
        <p>
          <strong>Reload</strong> to use external changes, or <strong>Keep Mine</strong> to save your current settings.
        </p>
        <div class="mt-4 ml-auto flex gap-2">
          <Button size="sm" onclick={() => settings.reloadFromFile()}>
            Reload
          </Button>
          <Button size="sm" variant="destructive" onclick={() => settings.dismissConfigChanged()}>
            Keep Mine
          </Button>
        </div>
      </Alert.Description>
    </Alert.Root>
  {/if}

  <AudioSettings />

  <Card.Root>
    <Card.Header>
      <Card.Title>Transcriptions</Card.Title>
    </Card.Header>
    <Card.Content class="space-y-8">
      <div class="flex flex-row gap-2 items-center justify-between">
        <div class="space-y-1">
          <Label for="output-mode">Output Mode</Label>
          <p class="text-sm text-muted-foreground">
            Choose how transcribed text should be handled after recording.
          </p>
        </div>
        <Select.Root
          type="single"
          bind:value={settings.outputMode}
          onValueChange={(mode) => settings.setOutputMode(mode as import('$lib/api/types').OutputMode)}
        >
          <Select.Trigger id="output-mode" class="w-[280px]">
            {getOutputModeLabel(settings.outputMode) || "Select output mode"}
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
      <div class="space-y-4">
        <div class="space-y-1">
          <Label for="trancription-model">Model</Label>
          <p class="text-sm text-muted-foreground">
            Select and manage transcription models.
          </p>
        </div>
        <RadioGroup.Root id="transcription-model">
          <div class="flex flex-col gap-2">
            <Label
              for="trancription-model-1"
              class={`group flex cursor-pointer items-start flex-col gap-3 rounded-lg border p-4 transition-colors hover:bg-muted/50 ${settings.osdPosition === 'top' ? 'ring-2 ring-primary bg-muted/30' : ''}`}
            >
              <div class="flex items-center gap-3">
                <RadioGroup.Item value="top" id="trancription-model-1" />
                <span class="font-medium cursor-pointer">Model 1</span>
              </div>
            </Label>
            <Label
              for="trancription-model-2"
              class={`group flex cursor-pointer items-start flex-col gap-3 rounded-lg border p-4 transition-colors hover:bg-muted/50 ${settings.osdPosition === 'top' ? 'ring-2 ring-primary bg-muted/30' : ''}`}
            >
              <div class="flex items-center gap-3">
                <RadioGroup.Item value="top" id="trancription-model-2" />
                <span class="font-medium cursor-pointer">Model 2</span>
              </div>
            </Label>
          </div>
        </RadioGroup.Root>
      </div>
    </Card.Content>
  </Card.Root>

  <Card.Root>
    <Card.Header>
      <Card.Title>Appearance</Card.Title>
      <Card.Description>Customize the appearance of the application</Card.Description>
    </Card.Header>
    <Card.Content class="space-y-8">
      <div class="space-y-4">
        <div class="space-y-1">
          <Label for="osd-position" class="font-medium">On-screen display</Label>
          <p class="text-sm text-muted-foreground">
            Choose where the on-screen display appears during recording.
          </p>
        </div>
        <RadioGroup.Root id="osd-position" bind:value={settings.osdPosition} onValueChange={(position) => settings.setOsdPosition(position as import('$lib/api/types').OsdPosition)}>
          <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
            <Label
              for="position-top"
              class={`group flex cursor-pointer items-start flex-col gap-3 rounded-lg border p-4 transition-colors hover:bg-muted/50 ${settings.osdPosition === 'top' ? 'ring-2 ring-primary bg-muted/30' : ''}`}
            >
              <div class="flex items-center gap-3">
                <RadioGroup.Item value="top" id="position-top" />
                <span class="font-medium cursor-pointer">Top</span>
              </div>
              <OsdPreview position="top" class="w-full h-auto rounded-sm border shadow-sm transition-shadow duration-200 group-hover:shadow-md" />
            </Label>
            <Label
              for="position-bottom"
              class={`group flex cursor-pointer items-start flex-col gap-3 rounded-lg border p-4 transition-colors hover:bg-muted/50 ${settings.osdPosition === 'bottom' ? 'ring-2 ring-primary bg-muted/30' : ''}`}
            >
              <div class="flex items-center gap-3">
                <RadioGroup.Item value="bottom" id="position-bottom" />
                <span class="font-medium cursor-pointer">Bottom</span>
              </div>
              <OsdPreview position="bottom" class="w-full h-auto rounded-sm border shadow-sm transition-shadow duration-200 group-hover:shadow-md" />
            </Label>
          </div>
        </RadioGroup.Root>
      </div>
      <div class="flex gap-2 items-center justify-between">
        <div class="space-y-1 flex-1">
          <Label for="window-decorations" class="font-medium">Show window titlebar</Label>
          <p class="text-sm text-muted-foreground">
            Display native window titlebar with minimize, maximize, and close buttons.
            Disable this for tiling window managers like Hyprland, i3, or sway.
          </p>
        </div>
        <Switch
          id="window-decorations"
          bind:checked={settings.windowDecorations}
          onCheckedChange={(enabled) => settings.setWindowDecorations(enabled)}
        />
      </div>
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
