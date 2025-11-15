<script lang="ts">
  import Page from "$lib/components/page.svelte";
  import * as Card from "$lib/components/ui/card";
  import MicIcon from "@lucide/svelte/icons/mic";
  import SquareIcon from "@lucide/svelte/icons/square";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  let status = $state("idle");
  let transcriptionText = $state("");
  let recording = $derived(status === "recording");
  let transcribing = $derived(status === "transcribing");

  onMount(() => {
    const unsubscribe1 = listen("recording-started", () => {
      status = "recording";
      transcriptionText = "";
    });

    const unsubscribe2 = listen("recording-stopped", () => {
      status = "transcribing";
    });

    const unsubscribe3 = listen("transcription-complete", () => {
      status = "idle";
    });

    const unsubscribe4 = listen("transcription-result", (event: any) => {
      transcriptionText = event.payload.text;
    });

    // Get initial status
    invoke("get_status").then((s) => {
      status = s as string;
    });

    return () => {
      unsubscribe1.then(f => f());
      unsubscribe2.then(f => f());
      unsubscribe3.then(f => f());
      unsubscribe4.then(f => f());
    };
  });

  async function toggle() {
    try {
      await invoke("toggle_recording");
    } catch (err) {
      console.error("Toggle failed:", err);
    }
  }
</script>

<Page class="items-center justify-center max-w-6xl">
  <div class="relative flex justify-center items-center my-8">
    {#if transcribing}
      <div class="w-[220px] h-[220px] rounded-full border-3 border-primary border-t-transparent animate-spin"></div>
    {:else}
      <button
        class="w-[200px] h-[200px] rounded-full border-none flex items-center justify-center transition-all duration-300 shadow-[0_10px_30px_rgba(0,0,0,0.2)] relative group cursor-pointer text-primary-foreground
          {recording ? 'bg-destructive animate-[pulse_2s_ease-in-out_infinite] hover:animate-none' : 'bg-primary hover:scale-105 hover:shadow-[0_15px_40px_rgba(0,0,0,0.3)] active:scale-[0.98]'}"
        onclick={toggle}
        aria-label={recording ? "Stop recording" : "Start recording"}
      >
        <MicIcon class="w-20 h-20 stroke-1.5 absolute transition-opacity duration-200 {recording ? 'group-hover:opacity-0' : ''}" />
        {#if recording}
          <SquareIcon class="w-20 h-20 stroke-1.5 fill-current absolute opacity-0 transition-opacity duration-200 group-hover:opacity-100" />
        {/if}
      </button>
    {/if}
  </div>

  {#if transcriptionText}
    <Card.Root class="w-full">
      <Card.Header>
        <Card.Title>Transcription</Card.Title>
      </Card.Header>
      <Card.Content>
        <p class="text-foreground whitespace-pre-wrap">{transcriptionText}</p>
      </Card.Content>
    </Card.Root>
  {/if}

  <div class="text-center opacity-70">
    <p class="my-2">Press the button or use your configured hotkey to toggle recording.</p>
    <p class="text-sm opacity-60 my-2">Tip: Bind a system hotkey to run: <code class="bg-muted px-2 py-1 rounded font-mono">dictate toggle</code></p>
    <p class="text-sm opacity-60 my-2">Configure output mode in <a href="/settings" class="text-primary hover:underline">Settings</a></p>
  </div>
</Page>

<style>
  @keyframes pulse {
    0%, 100% {
      opacity: 1;
      box-shadow: 0 10px 30px rgba(0, 0, 0, 0.2), 0 0 0 0 hsl(var(--destructive) / 0.4);
    }
    50% {
      opacity: 0.8;
      box-shadow: 0 10px 30px rgba(0, 0, 0, 0.2), 0 0 0 20px hsl(var(--destructive) / 0);
    }
  }
</style>
