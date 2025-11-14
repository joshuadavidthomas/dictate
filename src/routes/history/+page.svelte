<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { ask, message } from '@tauri-apps/plugin-dialog';
  import * as Card from "$lib/components/ui/card";
  import * as Button from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";

  interface TranscriptionHistory {
    id: number;
    text: string;
    created_at: number;
    duration_ms: number | null;
    model_name: string | null;
    audio_path: string | null;
    output_mode: string | null;
    audio_size_bytes: number | null;
  }

  let transcriptions: TranscriptionHistory[] = [];
  let loading = true;
  let error = '';
  let searchQuery = '';
  let totalCount = 0;

  async function loadTranscriptions() {
    loading = true;
    error = '';
    try {
      if (searchQuery.trim()) {
        transcriptions = await invoke<TranscriptionHistory[]>('search_transcription_history', {
          query: searchQuery,
          limit: 100
        });
      } else {
        transcriptions = await invoke<TranscriptionHistory[]>('get_transcription_history', {
          limit: 100,
          offset: 0
        });
      }
      totalCount = await invoke<number>('get_transcription_count');
    } catch (e) {
      error = `Failed to load transcriptions: ${e}`;
      console.error('Error loading transcriptions:', e);
    } finally {
      loading = false;
    }
  }

  async function deleteTranscription(id: number) {
    const confirmed = await ask('Are you sure you want to delete this transcription?', {
      title: 'Confirm Delete',
      kind: 'warning'
    });
    
    if (!confirmed) {
      return;
    }
    
    try {
      const deleted = await invoke<boolean>('delete_transcription_by_id', { id });
      if (deleted) {
        await loadTranscriptions();
      }
    } catch (e) {
      console.error('Error deleting transcription:', e);
      await message(`Failed to delete: ${e}`, {
        title: 'Error',
        kind: 'error'
      });
    }
  }

  function formatDate(timestamp: number): string {
    const date = new Date(timestamp * 1000);
    return date.toLocaleString();
  }

  function formatDuration(ms: number | null): string {
    if (!ms) return 'N/A';
    const seconds = Math.floor(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    const remainingSeconds = seconds % 60;
    return `${minutes}m ${remainingSeconds}s`;
  }

  function formatSize(bytes: number | null): string {
    if (!bytes) return 'N/A';
    if (bytes < 1024) return `${bytes}B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
  }

  function getModelDisplayName(modelName: string | null): string {
    if (!modelName) return 'Unknown';
    const parts = modelName.split('/');
    return parts[parts.length - 1] || modelName;
  }

  async function handleSearch() {
    await loadTranscriptions();
  }

  onMount(() => {
    loadTranscriptions();
  });
</script>

<div class="flex flex-1 flex-col gap-6 p-8">
  <div class="mx-auto w-full max-w-6xl space-y-6">
    <div>
      <h1 class="text-3xl font-bold mb-2">History</h1>
      <p class="text-muted-foreground">View and manage your past transcriptions ({totalCount} total)</p>
    </div>

    <div class="flex gap-2">
      <Input
        bind:value={searchQuery}
        placeholder="Search transcriptions..."
        on:keydown={(e) => e.key === 'Enter' && handleSearch()}
        class="flex-1"
      />
      <Button.Root onclick={handleSearch}>Search</Button.Root>
      {#if searchQuery}
        <Button.Root variant="outline" onclick={() => { searchQuery = ''; loadTranscriptions(); }}>
          Clear
        </Button.Root>
      {/if}
    </div>

    {#if loading}
      <Card.Root>
        <Card.Content class="pt-6">
          <div class="flex flex-col items-center justify-center py-12 text-center">
            <p class="text-muted-foreground">Loading transcriptions...</p>
          </div>
        </Card.Content>
      </Card.Root>
    {:else if error}
      <Card.Root>
        <Card.Content class="pt-6">
          <div class="flex flex-col items-center justify-center py-12 text-center">
            <p class="text-red-500">{error}</p>
            <Button.Root onclick={loadTranscriptions} class="mt-4">Retry</Button.Root>
          </div>
        </Card.Content>
      </Card.Root>
    {:else if transcriptions.length === 0}
      <Card.Root>
        <Card.Content class="pt-6">
          <div class="flex flex-col items-center justify-center py-12 text-center">
            <svg
              xmlns="http://www.w3.org/2000/svg"
              width="48"
              height="48"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              class="mb-4 text-muted-foreground"
            >
              <circle cx="12" cy="12" r="10"></circle>
              <polyline points="12 6 12 12 16 14"></polyline>
            </svg>
            <h3 class="mb-2 text-lg font-semibold">
              {searchQuery ? 'No matching transcriptions' : 'No transcriptions yet'}
            </h3>
            <p class="mb-4 text-sm text-muted-foreground max-w-sm">
              {searchQuery 
                ? 'Try a different search query' 
                : 'Your transcription history will appear here once you start recording.'}
            </p>
          </div>
        </Card.Content>
      </Card.Root>
    {:else}
      <div class="space-y-4">
        {#each transcriptions as transcription (transcription.id)}
          <Card.Root>
            <Card.Header>
              <div class="flex items-start justify-between">
                <div class="flex-1">
                  <Card.Title class="text-base">
                    {formatDate(transcription.created_at)}
                  </Card.Title>
                  <Card.Description class="text-xs mt-1">
                    Model: {getModelDisplayName(transcription.model_name)} • 
                    Duration: {formatDuration(transcription.duration_ms)} • 
                    Size: {formatSize(transcription.audio_size_bytes)} •
                    Output: {transcription.output_mode || 'N/A'}
                  </Card.Description>
                </div>
                <Button.Root 
                  variant="ghost" 
                  size="sm"
                  onclick={() => deleteTranscription(transcription.id)}
                  class="text-red-500 hover:text-red-700"
                >
                  Delete
                </Button.Root>
              </div>
            </Card.Header>
            <Card.Content>
              <p class="text-sm whitespace-pre-wrap">{transcription.text}</p>
            </Card.Content>
          </Card.Root>
        {/each}
      </div>
    {/if}
  </div>
</div>
