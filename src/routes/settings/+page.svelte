<script lang="ts">
  import Heading from "$lib/components/heading.svelte";
  import Page from "$lib/components/page.svelte";
  import {
      AudioSettings,
      SettingsRadioCards,
      SettingsRadioCardsItem,
      SettingsRadioGroup,
      SettingsRadioGroupItem,
      SettingsSection,
      SettingsSelect,
      SettingsSelectItem,
      SettingsSwitch
  } from "$lib/components/settings";
  import * as Alert from "$lib/components/ui/alert";
  import { Button } from "$lib/components/ui/button";
// import { Progress } from "$lib/components/ui/progress";
  import * as Card from "$lib/components/ui/card";
  import { settings } from "$lib/stores";
  import { getModelsState } from "$lib/stores/transcription-models-context.svelte";
  import { modelIdToString, modelKey } from "$lib/stores/transcription-models.svelte";
  import OsdPreview from "@/components/osd-preview.svelte";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import DownloadIcon from "@lucide/svelte/icons/download";
  import Loader2Icon from "@lucide/svelte/icons/loader-2";
  import TrashIcon from "@lucide/svelte/icons/trash";
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

  const modelsState = getModelsState();

  onMount(() => {
    const handleFocus = () => {
      settings.checkConfigChanged();
    };

    settings.load();

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
          bind:value={modelsState.preferredModelValue}
          onValueChange={modelsState.setPreferred}
        >
          {#snippet description()}
            Select and manage transcription models. Download a Parakeet or Whisper model to enable real transcription.
          {/snippet}

          {#if modelsState.initialLoading}
            <p class="text-sm text-muted-foreground">
              Loading models...
            </p>
          {/if}

          {#if modelsState.hasError}
            <p class="text-sm text-destructive">
              {modelsState.error}
            </p>
          {/if}

          {#if !modelsState.initialLoading && !modelsState.hasError && !modelsState.hasAnyModels}
            <p class="text-sm text-muted-foreground">
              No models are available yet.
            </p>
          {/if}


          {#if modelsState.parakeetModels.length}

            <div class="space-y-2">
              <p class="text-xs font-semibold text-muted-foreground">
                Parakeet models
              </p>

              {#each modelsState.parakeetModels as model (modelKey(model.id))}
                <SettingsRadioGroupItem
                  class="relative"
                  value={modelIdToString(model.id)}
                  disabled={!model.is_downloaded}
                >
                  <div class="flex w-full flex-col gap-1">
                    <div class="flex w-full items-center justify-between gap-4">
                      <div class="flex flex-col gap-1" class:text-muted-foreground={!model.is_downloaded}>
                        <span class="font-medium">
                          Parakeet {model.id.id}
                          {#if modelsState.isActiveModel(model) && model.is_downloaded}
                            <span class="ml-2 rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-700 dark:bg-green-900/40 dark:text-green-300">
                              Active
                            </span>
                          {/if}
                        </span>
                        {#if modelsState.formatModelSize(model.id)}
                          <span class="text-xs text-muted-foreground">
                            {modelsState.formatModelSize(model.id)}
                          </span>
                        {/if}
                      </div>

                      <div class="flex items-center gap-2">
                        {#if model.is_downloaded}
                          <Button
                            size="sm"
                            variant="destructive"
                            onclick={() => modelsState.remove(model)}
                            disabled={modelsState.removing[modelKey(model.id)]}
                          >
                             <TrashIcon class="mr-1 h-3 w-3" />
                             Delete

                          </Button>
                        {:else}
                          <Button
                            size="sm"
                            variant="ghost"
                            class="border border-transparent hover:border-border"
                            onclick={() => modelsState.download(model)}
                            disabled={modelsState.downloading[modelKey(model.id)]}
                          >
                            {#if modelsState.downloading[modelKey(model.id)]}
                              <Loader2Icon class="mr-1 h-3 w-3 animate-spin" />
                              Downloading
                            {:else}
                              <DownloadIcon class="mr-1 h-3 w-3" />
                              Download
                            {/if}
                          </Button>
                        {/if}
                      </div>
                    </div>

                    {#if modelsState.downloading[modelKey(model.id)] && modelsState.downloadProgress[modelKey(model.id)]}
                      {@const p = modelsState.downloadProgress[modelKey(model.id)]}
                      {@const percent =
                        p.totalBytes > 0
                          ? Math.round((p.downloadedBytes / p.totalBytes) * 100)
                          : 0}

                      <div class="pointer-events-none absolute inset-x-0 bottom-0 h-1 overflow-hidden rounded-b bg-muted/40">
                        <div
                          class="h-full bg-emerald-500 transition-[width] duration-150"
                          style={`width: ${percent}%`}
                        ></div>
                      </div>
                    {/if}
                  </div>
                </SettingsRadioGroupItem>
              {/each}
            </div>
          {/if}

          {#if modelsState.whisperModels.length}
            <div class="mt-6 space-y-2">
              <p class="text-xs font-semibold text-muted-foreground">
                Whisper models
              </p>

              {#each modelsState.whisperModels as model (modelKey(model.id))}
                <SettingsRadioGroupItem
                  class="relative"
                  value={modelIdToString(model.id)}
                  disabled={!model.is_downloaded}
                >
                  <div class="flex w-full flex-col gap-1">
                    <div class="flex w-full items-center justify-between gap-4">
                      <div class="flex flex-col gap-1" class:text-muted-foreground={!model.is_downloaded}>
                        <span class="font-medium">
                          Whisper {model.id.id}
                          {#if modelsState.isActiveModel(model) && model.is_downloaded}
                            <span class="ml-2 rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-700 dark:bg-green-900/40 dark:text-green-300">
                              Active
                            </span>
                          {/if}
                        </span>
                        {#if modelsState.formatModelSize(model.id)}
                          <span class="text-xs text-muted-foreground">
                            {modelsState.formatModelSize(model.id)}
                          </span>
                        {/if}
                      </div>

                      <div class="flex items-center gap-2">
                        {#if model.is_downloaded}
                          <Button
                            size="sm"
                            variant="destructive"
                            onclick={() => modelsState.remove(model)}
                            disabled={modelsState.removing[modelKey(model.id)]}
                          >
                             <TrashIcon class="mr-1 h-3 w-3" />
                             Delete

                          </Button>
                        {:else}
                          <Button
                            size="sm"
                            variant="ghost"
                            class="border border-transparent hover:border-border"
                            onclick={() => modelsState.download(model)}
                            disabled={modelsState.downloading[modelKey(model.id)]}
                          >
                            {#if modelsState.downloading[modelKey(model.id)]}
                              <Loader2Icon class="mr-1 h-3 w-3 animate-spin" />
                              Downloading
                            {:else}
                              <DownloadIcon class="mr-1 h-3 w-3" />
                              Download
                            {/if}
                          </Button>
                        {/if}
                      </div>
                    </div>

                    {#if modelsState.downloading[modelKey(model.id)] && modelsState.downloadProgress[modelKey(model.id)]}
                      {@const p = modelsState.downloadProgress[modelKey(model.id)]}
                      {@const percent =
                        p.totalBytes > 0
                          ? Math.round((p.downloadedBytes / p.totalBytes) * 100)
                          : 0}

                      <div class="pointer-events-none absolute inset-x-0 bottom-0 h-1 overflow-hidden rounded-b bg-muted/40">
                        <div
                          class="h-full bg-emerald-500 transition-[width] duration-150"
                          style={`width: ${percent}%`}
                        ></div>
                      </div>
                    {/if}
                  </div>
                </SettingsRadioGroupItem>
              {/each}
            </div>
          {/if}
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
