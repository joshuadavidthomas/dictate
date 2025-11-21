<script lang="ts">
  import { Button } from "$lib/components/ui/button";
  import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
  import CheckCircleIcon from "@lucide/svelte/icons/check-circle";
  import { invoke } from "@tauri-apps/api/core";

  interface Props {
    deviceName: string;
  }

  let { deviceName }: Props = $props();

  let testingDevice = $state(false);
  let deviceTestResult = $state<"success" | "error" | null>(null);
  let audioLevel = $state(0);
  let maxAudioLevel = $state(0);
  let noInputDetected = $state(false);
  let audioLevelInterval: number | null = null;

  async function testDevice() {
    testingDevice = true;
    deviceTestResult = null;
    audioLevel = 0;
    maxAudioLevel = 0;
    noInputDetected = false;

    try {
      const backendDeviceName = deviceName === "default" ? null : deviceName;

      // Backend test is connection-only: succeeds if we can open the device
      const ok = await invoke("test_audio_device", { deviceName: backendDeviceName }) as boolean;

      if (!ok) {
        deviceTestResult = "error";
        testingDevice = false;
        setTimeout(() => {
          deviceTestResult = null;
        }, 3000);
        return;
      }

      // Connection succeeded: show success immediately
      deviceTestResult = "success";

      // Start monitoring audio levels for visualization and signal presence
      audioLevelInterval = setInterval(async () => {
        try {
          const level = await invoke("get_audio_level", { deviceName: backendDeviceName }) as number;
          const scaled = Math.min(level * 10, 1.0); // Scale up and clamp to 0-1
          audioLevel = scaled;

          if (scaled > maxAudioLevel) {
            maxAudioLevel = scaled;
          }
        } catch (err) {
          console.error("Failed to get audio level:", err);
        }
      }, 100);

      // After a short window, stop and optionally warn about missing input
      const WINDOW_MS = 4000;
      setTimeout(() => {
        if (audioLevelInterval !== null) {
          clearInterval(audioLevelInterval);
          audioLevelInterval = null;
        }

        testingDevice = false;
        audioLevel = 0;

        const LEVEL_THRESHOLD = 0.1;
        if (maxAudioLevel < LEVEL_THRESHOLD) {
          noInputDetected = true;
        }

        setTimeout(() => {
          deviceTestResult = null;
          noInputDetected = false;
        }, 2000);
      }, WINDOW_MS);
    } catch (err) {
      console.error("Audio device test failed:", err);
      deviceTestResult = "error";
      testingDevice = false;
      setTimeout(() => {
        deviceTestResult = null;
      }, 3000);
    }
  }
</script>

<div class="flex items-center gap-3">
  <Button
    size="sm"
    variant="secondary"
    onclick={testDevice}
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
    <div class="flex flex-col gap-0.5 text-sm">
      <div class="flex items-center gap-1 text-green-600 dark:text-green-400">
        <CheckCircleIcon class="h-4 w-4" />
        <span>Device connected successfully</span>
      </div>
      {#if noInputDetected}
        <div class="flex items-center gap-1 text-amber-600 dark:text-amber-400">
          <AlertTriangleIcon class="h-4 w-4" />
          <span>No input detected – check mic mute or source</span>
        </div>
      {/if}
    </div>
  {:else if deviceTestResult === "error"}
    <div class="flex items-center gap-1 text-sm text-red-600 dark:text-red-400">
      <AlertTriangleIcon class="h-4 w-4" />
      <span>Device test failed</span>
    </div>
  {/if}
</div>
