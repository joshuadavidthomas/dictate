<script lang="ts">
  import Heading from "$lib/components/heading.svelte";
  import OsdPreview from "$lib/components/osd-preview.svelte";
  import Page from "$lib/components/page.svelte";
  import {
    AudioSettings,
    SettingsSection,
    SettingsSelect,
    SettingsSelectItem,
    SettingsSwitch,
    SettingsRadioCards,
    SettingsRadioCardsItem,
    SettingsRadioGroup,
    SettingsRadioGroupItem
  } from "$lib/components/settings";
  import * as Alert from "$lib/components/ui/alert";
  import { Button } from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
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
    <Card.Content>
      <SettingsSection>
        <SettingsSelect
          id="output-mode"
          label="Output Mode"
          bind:value={settings.outputMode}
          onValueChange={(mode) => settings.setOutputMode(mode as import('$lib/api/types').OutputMode)}
        >
          {#snippet trigger({ value })}
            {getOutputModeLabel(value) || "Select output mode"}
          {/snippet}
          {#snippet description()}
            Choose how transcribed text should be handled after recording.
          {/snippet}
          
          {#each outputModeOptions as option}
            <SettingsSelectItem value={option.value} label={option.label}>
              <div class="flex flex-col gap-1">
                <span class="font-medium">{option.label}</span>
                <span class="text-xs text-muted-foreground">{option.description}</span>
              </div>
            </SettingsSelectItem>
          {/each}
        </SettingsSelect>
        
        <SettingsRadioGroup
          id="transcription-model"
          label="Model"
        >
          {#snippet description()}
            Select and manage transcription models.
          {/snippet}
          
          <SettingsRadioGroupItem value="model1">
            Model 1
          </SettingsRadioGroupItem>
          <SettingsRadioGroupItem value="model2">
            Model 2
          </SettingsRadioGroupItem>
        </SettingsRadioGroup>
      </SettingsSection>
    </Card.Content>
  </Card.Root>

  <Card.Root>
    <Card.Header>
      <Card.Title>Appearance</Card.Title>
      <Card.Description>Customize the appearance of the application</Card.Description>
    </Card.Header>
    <Card.Content>
      <SettingsSection>
        <SettingsRadioCards
          id="osd-position"
          label="On-screen display"
          bind:value={settings.osdPosition}
          onValueChange={(position) => settings.setOsdPosition(position as import('$lib/api/types').OsdPosition)}
        >
          {#snippet description()}
            Choose where the on-screen display appears during recording.
          {/snippet}
          
          <SettingsRadioCardsItem value="top">
            Top
            {#snippet preview()}
              <OsdPreview position="top" class="w-full h-auto rounded-sm border shadow-sm transition-shadow duration-200 group-hover:shadow-md" />
            {/snippet}
          </SettingsRadioCardsItem>
          
          <SettingsRadioCardsItem value="bottom">
            Bottom
            {#snippet preview()}
              <OsdPreview position="bottom" class="w-full h-auto rounded-sm border shadow-sm transition-shadow duration-200 group-hover:shadow-md" />
            {/snippet}
          </SettingsRadioCardsItem>
        </SettingsRadioCards>
        
        <SettingsSwitch
          id="window-decorations"
          label="Show window titlebar"
          bind:checked={settings.windowDecorations}
          onCheckedChange={(enabled) => settings.setWindowDecorations(enabled)}
        >
          {#snippet description()}
            Display native window titlebar with minimize, maximize, and close buttons. Disable this for tiling window managers like Hyprland, i3, or sway.
          {/snippet}
        </SettingsSwitch>
      </SettingsSection>
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
