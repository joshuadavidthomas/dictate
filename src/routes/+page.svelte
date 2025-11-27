<script lang="ts">
  import Page from "$lib/components/page.svelte";
  import * as Card from "$lib/components/ui/card";
  import { getRecordingState } from "$lib/stores";
  import MicIcon from "@lucide/svelte/icons/mic";
  import SquareIcon from "@lucide/svelte/icons/square";

  const recording = getRecordingState();

  async function toggle() {
    try {
      await recording.toggle();
    } catch (err) {
      console.error("Toggle failed:", err);
    }
  }
</script>

<Page class="items-center justify-center">
  <div class="relative flex justify-center items-center my-8">
    {#if recording.isTranscribing}
      <div class="w-[220px] h-[220px] rounded-full border-3 border-primary border-t-transparent animate-spin"></div>
    {:else}
      <button
        class="w-[200px] h-[200px] rounded-full border-none flex items-center justify-center transition-all duration-300 shadow-[0_10px_30px_rgba(0,0,0,0.2)] relative group cursor-pointer text-primary-foreground
          {recording.isRecording ? 'bg-destructive animate-[pulse_2s_ease-in-out_infinite] hover:animate-none' : 'bg-primary hover:scale-105 hover:shadow-[0_15px_40px_rgba(0,0,0,0.3)] active:scale-[0.98]'}"
        onclick={toggle}
        aria-label={recording.isRecording ? "Stop recording" : "Start recording"}
      >
        <MicIcon class="w-20 h-20 stroke-1.5 absolute transition-opacity duration-200 {recording.isRecording ? 'group-hover:opacity-0' : ''}" />
        {#if recording.isRecording}
          <SquareIcon class="w-20 h-20 stroke-1.5 fill-current absolute opacity-0 transition-opacity duration-200 group-hover:opacity-100" />
        {/if}
      </button>
    {/if}
  </div>

  {#if recording.transcriptionText}
    <Card.Root class="w-full">
      <Card.Header>
        <Card.Title>Transcription</Card.Title>
      </Card.Header>
      <Card.Content>
        <p class="text-foreground whitespace-pre-wrap">{recording.transcriptionText}</p>
      </Card.Content>
    </Card.Root>
  {/if}
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
