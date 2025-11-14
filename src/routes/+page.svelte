<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import { Button } from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";

  let status = $state("idle");
  let transcriptionText = $state("");
  let recording = $derived(status === "recording");
  let transcribing = $derived(status === "transcribing");

  onMount(() => {
    // Listen for status updates from Rust
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

<div class="flex flex-1 flex-col gap-4 p-8 items-center justify-center">
  <div class="container mx-auto">
  <div class="status">
    <div class="status-indicator" class:recording class:transcribing>
      {#if recording}
        Recording...
      {:else if transcribing}
        Transcribing...
      {:else}
        Ready
      {/if}
    </div>
  </div>

  <Button 
    size="lg"
    variant={recording ? "destructive" : "default"}
    onclick={toggle}
    disabled={transcribing}
    class="w-full max-w-xs"
  >
    {#if recording}
      Stop Recording
    {:else if transcribing}
      Processing...
    {:else}
      Start Recording
    {/if}
  </Button>

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

    <div class="info">
      <p>Press the button or use your configured hotkey to toggle recording.</p>
      <p class="hint">Tip: Bind a system hotkey to run: <code>dictate toggle</code></p>
      <p class="hint">Configure output mode in <a href="/settings" class="text-primary hover:underline">Settings</a></p>
    </div>
  </div>
</div>

<style>
  .container {
    max-width: 600px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2rem;
  }

  .status {
    width: 100%;
    display: flex;
    justify-content: center;
    padding: 2rem 0;
  }

  .status-indicator {
    padding: 1rem 2rem;
    border-radius: 8px;
    border: 2px solid hsl(var(--border));
    font-size: 1.2rem;
    font-weight: 500;
    transition: all 0.3s ease;
  }

  .status-indicator.recording {
    background: hsl(var(--destructive) / 0.1);
    border-color: hsl(var(--destructive));
    color: hsl(var(--destructive));
    animation: pulse 2s ease-in-out infinite;
  }

  .status-indicator.transcribing {
    background: hsl(var(--primary) / 0.1);
    border-color: hsl(var(--primary));
    color: hsl(var(--primary));
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.5; }
  }

  .info {
    text-align: center;
    opacity: 0.7;
    max-width: 500px;
  }

  .info p {
    margin: 0.5rem 0;
  }

  .hint {
    font-size: 0.9rem;
    opacity: 0.6;
  }

  code {
    background: hsl(var(--muted));
    padding: 0.2rem 0.5rem;
    border-radius: 4px;
    font-family: 'Monaco', 'Courier New', monospace;
  }
</style>
