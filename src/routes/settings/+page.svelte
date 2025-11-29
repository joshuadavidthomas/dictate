<script lang="ts">
  import Heading from "$lib/components/heading.svelte";
  import Page from "$lib/components/page.svelte";
  import {
      AudioSettings,
      SettingsModelGroup,
      SettingsRadioCards,
      SettingsRadioCardsItem,
      SettingsSection,
      SettingsSelect,
      SettingsSelectItem,
      SettingsShortcutInput,
      SettingsSwitch
  } from "$lib/components/settings";
  import * as Alert from "$lib/components/ui/alert";
  import { Button } from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
  import * as Kbd from "$lib/components/ui/kbd";
  import { getAppSettingsState } from "$lib/stores";
  import OsdPreview from "@/components/osd-preview.svelte";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";

  const settings = getAppSettingsState();

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

  // Check config changes when window gains focus
  $effect(() => {
    const handleFocus = () => {
      settings.checkConfigChanged();
    };

    window.addEventListener('focus', handleFocus);

    return () => {
      window.removeEventListener('focus', handleFocus);
    };
  });
</script>

<Page>
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
      <Card.Title class="text-lg">Transcriptions</Card.Title>
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

        <Card.Root class="border-none shadow-none p-0 gap-2">
          <Card.Header class="p-0">
            <Card.Title>Models</Card.Title>
          </Card.Header>
          <Card.Content class="space-y-4 p-0">
            <SettingsModelGroup familyName="Moonshine" />
            <SettingsModelGroup familyName="Parakeet TDT" />
            <SettingsModelGroup familyName="Whisper" />
          </Card.Content>
        </Card.Root>

       </SettingsSection>
     </Card.Content>
   </Card.Root>

  <Card.Root>
    <Card.Header>
      <Card.Title class="text-lg">Appearance</Card.Title>
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
              <div class="flex justify-center">
                <OsdPreview position="top" class="w-full h-auto rounded-sm border shadow-sm transition-shadow duration-200 group-hover:shadow-md" />
              </div>
            {/snippet}
          </SettingsRadioCardsItem>

          <SettingsRadioCardsItem value="bottom">
            Bottom
            {#snippet preview()}
              <div class="flex justify-center">
                <OsdPreview position="bottom" class="w-full h-auto rounded-sm border shadow-sm transition-shadow duration-200 group-hover:shadow-md" />
              </div>
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
      <Card.Title class="text-lg">Keyboard Shortcuts</Card.Title>
      <Card.Description>Configure global hotkeys for recording</Card.Description>
    </Card.Header>
    <Card.Content>
      <SettingsSection>
        <SettingsShortcutInput
          id="global-shortcut"
          label="Toggle Recording"
          bind:value={settings.shortcut}
          onChange={(shortcut) => settings.setShortcut(shortcut)}
        >
          {#snippet description()}
            Global keyboard shortcut to start and stop recording. Works even when the app is minimized.
          {/snippet}
        </SettingsShortcutInput>

        {#if settings.shortcutCapabilities && (!settings.shortcutCapabilities.canRegister || settings.shortcutCapabilities.compositor === 'hyprland')}
          <Alert.Root class="mt-4 border-yellow-500 bg-yellow-50 text-yellow-900 dark:bg-yellow-950 dark:text-yellow-100">
            <AlertTriangleIcon />
            <Alert.Title>Additional compositor setup required</Alert.Title>
            <Alert.Description class="space-y-2">
              {#if settings.shortcutCapabilities.compositor === 'hyprland'}
                <p>
                  Hyprland requires a compositor binding for
                  <Kbd.Group class="inline-flex">
                    <Kbd.Root>Super</Kbd.Root>
                    <span class="text-yellow-700 dark:text-yellow-300">+</span>
                    <Kbd.Root>Shift</Kbd.Root>
                    <span class="text-yellow-700 dark:text-yellow-300">+</span>
                    <Kbd.Root>Space</Kbd.Root>
                  </Kbd.Group>.
                  Add this to your config:
                </p>
                <pre class="p-2 bg-yellow-100 dark:bg-yellow-900 rounded text-sm font-mono overflow-x-auto">bind = SUPER SHIFT, Space, global, transient:toggle</pre>
                <p class="text-sm">
                  Then reload with
                  <Kbd.Root class="bg-yellow-100 dark:bg-yellow-900 text-yellow-900 dark:text-yellow-100">hyprctl reload</Kbd.Root>
                </p>
              {:else if settings.shortcutCapabilities.compositor === 'sway'}
                <p>
                  Add a binding for
                  <Kbd.Group class="inline-flex">
                    <Kbd.Root>$mod</Kbd.Root>
                    <span class="text-yellow-700 dark:text-yellow-300">+</span>
                    <Kbd.Root>Shift</Kbd.Root>
                    <span class="text-yellow-700 dark:text-yellow-300">+</span>
                    <Kbd.Root>Space</Kbd.Root>
                  </Kbd.Group>
                  to your Sway config:
                </p>
                <pre class="p-2 bg-yellow-100 dark:bg-yellow-900 rounded text-sm font-mono overflow-x-auto">bindsym $mod+Shift+Space exec dictate toggle</pre>
}                <p class="text-sm">Then reload Sway config.</p>
              {:else if settings.shortcutCapabilities.compositor}
                <p>Global shortcuts require compositor configuration. Add a keybinding to run:</p>
                <pre class="p-2 bg-yellow-100 dark:bg-yellow-900 rounded text-sm font-mono overflow-x-auto">dictate toggle</pre>
              {:else}
                <p>Global shortcuts are not supported on this platform. Configure a shortcut in your window manager to run:</p>
                <pre class="p-2 bg-yellow-100 dark:bg-yellow-900 rounded text-sm font-mono overflow-x-auto">dictate toggle</pre>
              {/if}
            </Alert.Description>
          </Alert.Root>
        {/if}
      </SettingsSection>
    </Card.Content>
  </Card.Root>
</Page>
