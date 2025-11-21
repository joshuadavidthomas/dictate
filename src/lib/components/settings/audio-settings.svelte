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
  import { getAudioSettingsState } from "$lib/stores/audio-settings.svelte";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import InfoIcon from "@lucide/svelte/icons/info";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";

  const audioSettings = getAudioSettingsState();

  function getDeviceLabel(value: string): string {
    if (value === "default") return "System Default";
    return value;
  }

  function getSampleRateLabel(rate: string): string {
    const option = audioSettings.availableSampleRates.find(opt => opt.value === parseInt(rate));
    return option ? `${option.label} (${option.description})` : `${rate} Hz`;
  }

  function isSampleRateCompatible(sampleRate: number, availableRates: typeof audioSettings.availableSampleRates): boolean {
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
          value={audioSettings.currentDevice}
          onValueChange={audioSettings.setDevice}
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
                    onclick={() => audioSettings.loadDevices()}
                    disabled={audioSettings.isLoadingDevices}
                    class="shrink-0"
                  >
                    <RefreshCwIcon class={`h-4 w-4 ${audioSettings.isLoadingDevices ? 'animate-spin' : ''}`} />
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
          {#each audioSettings.availableDevices as deviceName}
            <SettingsSelectItem value={deviceName} label={deviceName}>
              <div class="flex flex-col gap-1">
                <span class="font-medium">{deviceName}</span>
              </div>
            </SettingsSelectItem>
          {/each}
        </SettingsSelect>

        <!-- Test Device Button -->
        <AudioDeviceTest deviceName={audioSettings.currentDevice} />
      </div>

      <!-- Sample Rate Selection -->
      <SettingsSelect
        id="sample-rate"
        label="Sample Rate"
        value={audioSettings.currentSampleRate.toString()}
        onValueChange={(v) => audioSettings.setSampleRate(parseInt(v))}
      >
        {#snippet trigger({ value })}
          {getSampleRateLabel(value)}
        {/snippet}
        {#snippet description()}
          Higher sample rates provide better quality but larger file sizes
        {/snippet}

        {#each audioSettings.availableSampleRates as option}
          <SettingsSelectItem value={option.value.toString()} label={option.label}>
            <div class="flex flex-col gap-1">
              <span class="font-medium">{option.label} - {option.description}</span>
            </div>
          </SettingsSelectItem>
        {/each}
      </SettingsSelect>

      <!-- Warning if incompatible sample rate -->
      {#if !isSampleRateCompatible(audioSettings.currentSampleRate, audioSettings.availableSampleRates)}
        <Alert.Root variant="destructive">
          <AlertTriangleIcon class="h-4 w-4" />
          <Alert.Title>Incompatible Sample Rate</Alert.Title>
          <Alert.Description>
            The current sample rate ({audioSettings.currentSampleRate} Hz) is not supported by this device.
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
