<script lang="ts">
  import {
      SettingsSection,
      SettingsSelect,
      SettingsSelectItem
  } from "$lib/components/settings";
  import AudioDeviceTest from "$lib/components/settings/audio-device-test.svelte";
  import * as Alert from "$lib/components/ui/alert";
  import { Button } from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
  import * as Tooltip from "$lib/components/ui/tooltip";
  import { getAppSettingsState } from "$lib/stores";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import InfoIcon from "@lucide/svelte/icons/info";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  const settings = getAppSettingsState();

  function getDeviceLabel(value: string): string {
    if (value === "default") return "System Default";
    return value;
  }

  function formatSampleRate(rate: number): string {
    const khz = rate / 1000;
    return khz % 1 === 0 ? `${khz} kHz` : `${khz.toFixed(1)} kHz`;
  }

  function isSampleRateCompatible(sampleRate: number, availableRates: typeof settings.availableSampleRates): boolean {
    return availableRates.some(opt => opt.value === sampleRate);
  }
</script>

<Card.Root>
  <Card.Header>
    <Card.Title class="text-lg">Audio</Card.Title>
    <Card.Description>Configure audio input and recording preferences</Card.Description>
  </Card.Header>
  <Card.Content>
    <SettingsSection>
      <!-- Audio Device Selection -->
      <div class="space-y-4">
        <SettingsSelect
          id="audio-device"
          label="Input Device"
          value={settings.currentDevice}
          onValueChange={settings.setDevice}
        >
          {#snippet trigger({ value })}
            {getDeviceLabel(value)}
          {/snippet}
          {#snippet action()}
            <Tooltip.Root>
              <Tooltip.Trigger>
                {#snippet child({ props })}
                  <Button
                    {...props}
                    size="icon"
                    variant="outline"
                    onclick={() => settings.loadDevices()}
                    disabled={settings.isLoadingDevices}
                    class="shrink-0"
                  >
                    <RefreshCwIcon class={`h-4 w-4 ${settings.isLoadingDevices ? 'animate-spin' : ''}`} />
                  </Button>
                {/snippet}
              </Tooltip.Trigger>
              <Tooltip.Content>
                <p>Refresh device list</p>
              </Tooltip.Content>
            </Tooltip.Root>
          {/snippet}

          <SettingsSelectItem value="default" label="System Default">
            System Default
          </SettingsSelectItem>
          {#each settings.availableDevices as deviceName}
            <SettingsSelectItem value={deviceName} label={deviceName}>
              <div class="flex flex-col gap-1">
                <span class="font-medium">{deviceName}</span>
              </div>
            </SettingsSelectItem>
          {/each}
        </SettingsSelect>

        <!-- Test Device Button -->
        <AudioDeviceTest deviceName={settings.currentDevice} />
      </div>

      <!-- Sample Rate Selection -->
      <SettingsSelect
        id="sample-rate"
        label="Sample Rate"
        value={settings.currentSampleRate.toString()}
        onValueChange={(v) => settings.setSampleRate(parseInt(v))}
      >
        {#snippet trigger({ value })}
          {formatSampleRate(parseInt(value))}
        {/snippet}
        {#snippet description()}
          Higher sample rates provide better quality but larger file sizes
        {/snippet}

        {#each settings.availableSampleRates as option}
          <SettingsSelectItem value={option.value.toString()} label={formatSampleRate(option.value)}>
            <div class="flex items-center gap-2">
              <span class="font-medium">{formatSampleRate(option.value)}</span>
              {#if option.is_recommended}
                <span class="text-xs text-muted-foreground">(Recommended)</span>
              {/if}
            </div>
          </SettingsSelectItem>
        {/each}
      </SettingsSelect>

      <!-- Warning if incompatible sample rate -->
      {#if !isSampleRateCompatible(settings.currentSampleRate, settings.availableSampleRates)}
        <Alert.Root variant="destructive">
          <AlertTriangleIcon class="h-4 w-4" />
          <Alert.Title>Incompatible Sample Rate</Alert.Title>
          <Alert.Description>
            The current sample rate ({settings.currentSampleRate} Hz) is not supported by this device.
            Please select a supported rate from the list above.
          </Alert.Description>
        </Alert.Root>
      {/if}
    </SettingsSection>

    <Alert.Root>
      <InfoIcon class="h-4 w-4" />
      <Alert.Title>Note</Alert.Title>
      <Alert.Description>
        Audio settings will apply to new recording sessions. Using 16 kHz is recommended for optimal transcription quality with Whisper models.
      </Alert.Description>
    </Alert.Root>
  </Card.Content>
</Card.Root>
