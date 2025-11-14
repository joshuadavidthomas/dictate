<script lang="ts">
  import * as Card from "$lib/components/ui/card";
  import * as Select from "$lib/components/ui/select";
  import * as Tooltip from "$lib/components/ui/tooltip";
  import * as Alert from "$lib/components/ui/alert";
  import { Button } from "$lib/components/ui/button";
  import { Label } from "$lib/components/ui/label";
  import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
  import CheckCircleIcon from "@lucide/svelte/icons/check-circle";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import InfoIcon from "@lucide/svelte/icons/info";
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";

  type AudioDevice = {
    name: string;
    is_default: boolean;
    supported_sample_rates: number[];
  };

  type SampleRateOption = {
    value: number;
    label: string;
    description: string;
    is_recommended: boolean;
  };

  let audioDevices = $state<AudioDevice[]>([]);
  let selectedAudioDevice = $state<string>("default");
  let sampleRate = $state<string>("16000");
  let sampleRateOptions = $state<SampleRateOption[]>([]);
  let loadingDevices = $state(false);
  let testingDevice = $state(false);
  let deviceTestResult = $state<"success" | "error" | null>(null);
  let audioLevel = $state(0);
  let audioLevelInterval: number | null = null;

  async function loadAudioDevices() {
    loadingDevices = true;
    try {
      audioDevices = await invoke("list_audio_devices") as AudioDevice[];
      // Ensure spinner shows for at least one full rotation (500ms)
      await new Promise(resolve => setTimeout(resolve, 500));
    } catch (err) {
      console.error("Failed to load audio devices:", err);
    } finally {
      loadingDevices = false;
    }
  }

  async function handleAudioDeviceChange() {
    try {
      const deviceName = selectedAudioDevice === "default" ? null : selectedAudioDevice;
      await invoke("set_audio_device", { deviceName });
    } catch (err) {
      console.error("Failed to set audio device:", err);
    }
  }

  async function handleSampleRateChange() {
    try {
      await invoke("set_sample_rate", { sampleRate: parseInt(sampleRate) });
    } catch (err) {
      console.error("Failed to set sample rate:", err);
    }
  }

  async function testAudioDevice() {
    testingDevice = true;
    deviceTestResult = null;
    audioLevel = 0;
    
    try {
      const deviceName = selectedAudioDevice === "default" ? null : selectedAudioDevice;
      
      // Test device initialization
      await invoke("test_audio_device", { deviceName });
      deviceTestResult = "success";
      
      // Start monitoring audio levels
      audioLevelInterval = setInterval(async () => {
        try {
          const level = await invoke("get_audio_level", { deviceName }) as number;
          audioLevel = Math.min(level * 10, 1.0); // Scale up and clamp to 0-1
        } catch (err) {
          console.error("Failed to get audio level:", err);
        }
      }, 100);
      
      // Stop after 5 seconds
      setTimeout(() => {
        if (audioLevelInterval !== null) {
          clearInterval(audioLevelInterval);
          audioLevelInterval = null;
        }
        testingDevice = false;
        audioLevel = 0;
        setTimeout(() => {
          deviceTestResult = null;
        }, 1000);
      }, 5000);
    } catch (err) {
      console.error("Audio device test failed:", err);
      deviceTestResult = "error";
      testingDevice = false;
      setTimeout(() => {
        deviceTestResult = null;
      }, 3000);
    }
  }

  function getAudioDeviceLabel(deviceName: string): string {
    if (deviceName === "default") return "System Default";
    const device = audioDevices.find(d => d.name === deviceName);
    return device ? device.name : deviceName;
  }

  function getSampleRateLabel(rate: string): string {
    const option = sampleRateOptions.find(opt => opt.value === parseInt(rate));
    return option ? `${option.label} (${option.description})` : `${rate} Hz`;
  }

  onMount(async () => {
    // Load devices and settings
    await loadAudioDevices();
    
    const device = await invoke("get_audio_device") as string | null;
    selectedAudioDevice = device ?? "default";
    
    const rate = await invoke("get_sample_rate") as number;
    sampleRate = rate.toString();
    
    const options = await invoke("get_sample_rate_options") as SampleRateOption[];
    sampleRateOptions = options;
  });
</script>

<Card.Root>
  <Card.Header>
    <Card.Title>Audio Settings</Card.Title>
    <Card.Description>Configure audio input and recording preferences</Card.Description>
  </Card.Header>
  <Card.Content class="space-y-6">
    <!-- Audio Device Selection -->
    <div class="space-y-4">
      <div class="flex items-center justify-between">
        <Label for="audio-device">Input Device</Label>
        <div class="flex gap-2">
          <Tooltip.Root>
            <Tooltip.Trigger>
              {#snippet child({ props })}
                <Button
                  {...props}
                  size="icon"
                  variant="outline"
                  onclick={loadAudioDevices}
                  disabled={loadingDevices}
                  class="shrink-0"
                >
                  <RefreshCwIcon class={`h-4 w-4 ${loadingDevices ? 'animate-spin' : ''}`} />
                </Button>
              {/snippet}
            </Tooltip.Trigger>
            <Tooltip.Content>
              <p>Refresh device list</p>
            </Tooltip.Content>
          </Tooltip.Root>
          <Select.Root
            type="single"
            bind:value={selectedAudioDevice}
            onValueChange={handleAudioDeviceChange}
          >
            <Select.Trigger id="audio-device" class="w-[280px]">
              {getAudioDeviceLabel(selectedAudioDevice)}
            </Select.Trigger>
            <Select.Content>
              <Select.Item value="default" label="System Default">
                <div class="flex items-center gap-2">
                  <span>System Default</span>
                  {#if audioDevices.find(d => d.is_default)}
                    <span class="text-xs text-muted-foreground">
                      ({audioDevices.find(d => d.is_default)?.name})
                    </span>
                  {/if}
                </div>
              </Select.Item>
              {#each audioDevices.filter(d => !d.is_default) as device}
                <Select.Item value={device.name} label={device.name}>
                  <div class="flex flex-col gap-1">
                    <span class="font-medium">{device.name}</span>
                  </div>
                </Select.Item>
              {/each}
            </Select.Content>
          </Select.Root>
        </div>
      </div>
      
      <!-- Test Device Button -->
      <div class="flex items-center gap-3">
        <Button
          size="sm"
          variant="secondary"
          onclick={testAudioDevice}
          disabled={testingDevice}
        >
          {testingDevice ? "Testing..." : "Test Device"}
        </Button>
        
        {#if testingDevice}
          <div class="flex items-center gap-0.5 h-6">
            {#each Array(20) as _, i}
              <div 
                class="w-1.5 h-full rounded-sm transition-all duration-75"
                class:bg-green-500={i < Math.floor(audioLevel * 20)}
                class:bg-muted={i >= Math.floor(audioLevel * 20)}
              ></div>
            {/each}
          </div>
        {:else if deviceTestResult === "success"}
          <div class="flex items-center gap-1 text-sm text-green-600 dark:text-green-400">
            <CheckCircleIcon class="h-4 w-4" />
            <span>Device is working</span>
          </div>
        {:else if deviceTestResult === "error"}
          <div class="flex items-center gap-1 text-sm text-red-600 dark:text-red-400">
            <AlertTriangleIcon class="h-4 w-4" />
            <span>Device test failed</span>
          </div>
        {/if}
      </div>
    </div>

    <!-- Sample Rate Selection -->
    <div class="flex items-center justify-between">
      <div class="space-y-1">
        <Label for="sample-rate">Sample Rate</Label>
        <p class="text-sm text-muted-foreground">
          Higher sample rates provide better quality but larger file sizes
        </p>
      </div>
      <Select.Root
        type="single"
        bind:value={sampleRate}
        onValueChange={handleSampleRateChange}
      >
        <Select.Trigger id="sample-rate" class="w-[280px]">
          {getSampleRateLabel(sampleRate)}
        </Select.Trigger>
        <Select.Content>
          {#each sampleRateOptions as option}
            <Select.Item value={option.value.toString()} label={option.label}>
              <div class="flex flex-col gap-1">
                <span class="font-medium">{option.label} - {option.description}</span>
              </div>
            </Select.Item>
          {/each}
        </Select.Content>
      </Select.Root>
    </div>

    <Alert.Root>
      <InfoIcon class="h-4 w-4" />
      <Alert.Title>Note</Alert.Title>
      <Alert.Description>
        Audio settings will apply to new recording sessions. Using 16 kHz is recommended for optimal transcription quality with Whisper models.
      </Alert.Description>
    </Alert.Root>
  </Card.Content>
</Card.Root>
