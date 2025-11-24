<script lang="ts">
  import Heading from "$lib/components/heading.svelte";
  import Page from "$lib/components/page.svelte";
  import * as Button from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
  import { Input } from "$lib/components/ui/input";
  import { getTranscriptionsState } from "$lib/stores";
  import { formatDate, formatDuration, formatSize } from '$lib/utils';
  import TrashIcon from "@lucide/svelte/icons/trash";
  import { ask, message } from '@tauri-apps/plugin-dialog';

  const transcriptions = getTranscriptionsState();

  async function deleteTranscription(id: number) {
    const confirmed = await ask('Are you sure you want to delete this transcription?', {
      title: 'Confirm Delete',
      kind: 'warning'
    });

    if (!confirmed) {
      return;
    }

    try {
      await transcriptions.delete(id);
    } catch (e) {
      console.error('Error deleting transcription:', e);
      await message(`Failed to delete: ${e}`, {
        title: 'Error',
        kind: 'error'
      });
    }
  }

  function getModelDisplayName(modelName: string | null): string {
    if (!modelName) return 'Unknown';
    const parts = modelName.split('/');
    return parts[parts.length - 1] || modelName;
  }

  async function handleSearch() {
    await transcriptions.load();
  }

  async function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter') {
      await handleSearch();
    }
  }
</script>

<Page class="mx-auto max-w-6xl">
  <div>
    <Heading>History</Heading>
    <p class="text-muted-foreground">View and manage your past transcriptions ({transcriptions.totalCount} total)</p>
  </div>

  <div class="flex gap-2">
    <Input
      bind:value={transcriptions.searchQuery}
      placeholder="Search transcriptions..."
      onkeydown={handleKeydown}
      class="flex-1"
    />
    <Button.Root onclick={handleSearch}>Search</Button.Root>
    {#if transcriptions.searchQuery}
      <Button.Root variant="outline" onclick={() => transcriptions.clearSearch()}>
        Clear
      </Button.Root>
    {/if}
  </div>

  {#if transcriptions.loading}
    <Card.Root>
      <Card.Content class="pt-6">
        <div class="flex flex-col items-center justify-center py-12 text-center">
          <p class="text-muted-foreground">Loading transcriptions...</p>
        </div>
      </Card.Content>
    </Card.Root>
  {:else if transcriptions.error}
    <Card.Root>
      <Card.Content class="pt-6">
        <div class="flex flex-col items-center justify-center py-12 text-center">
          <p class="text-red-500">{transcriptions.error}</p>
          <Button.Root onclick={() => transcriptions.load()} class="mt-4">Retry</Button.Root>
        </div>
      </Card.Content>
    </Card.Root>
  {:else if transcriptions.items.length === 0}
    <Card.Root>
      <Card.Content class="flex flex-col items-center justify-center py-12 text-center">
          <h3 class="mb-2 text-lg font-semibold">
            {transcriptions.searchQuery ? 'No matching transcriptions' : 'No transcriptions yet'}
          </h3>
          <p class="mb-4 text-sm text-muted-foreground max-w-sm">
            {transcriptions.searchQuery
              ? 'Try a different search query'
              : 'Your transcription history will appear here once you start recording.'}
          </p>
      </Card.Content>
    </Card.Root>
  {:else}
    <div class="space-y-4">
      {#each transcriptions.items as transcription (transcription.id)}
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
                variant="destructive"
                size="sm"
                onclick={() => deleteTranscription(transcription.id)}
                class="hover:opacity-80 hover:cursor-pointer"
              >
                <TrashIcon />
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
</Page>
