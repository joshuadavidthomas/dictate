<script lang="ts">
  import Heading from "$lib/components/heading.svelte";
  import Page from "$lib/components/page.svelte";
  import * as Button from "$lib/components/ui/button";
  import * as Card from "$lib/components/ui/card";
  import { Input } from "$lib/components/ui/input";
  import { getTranscriptionsState } from "$lib/stores";
  import { formatDate, formatDuration, formatSize } from '$lib/utils';
  import * as Tooltip from "$lib/components/ui/tooltip";
  import TrashIcon from "@lucide/svelte/icons/trash";
  import CopyIcon from "@lucide/svelte/icons/copy";
  import CheckIcon from "@lucide/svelte/icons/check";
  import { ask, message } from '@tauri-apps/plugin-dialog';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import type { ModelId } from '$lib/api/types';

  const transcriptions = getTranscriptionsState();
  let copiedId = $state<number | null>(null);
  let copyResetHandle: ReturnType<typeof setTimeout> | null = null;

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
 
  async function copyTranscription(id: number, text: string) {
    try {
      await writeText(text);
      copiedId = id;
      if (copyResetHandle) {
        clearTimeout(copyResetHandle);
      }
      copyResetHandle = setTimeout(() => {
        copiedId = null;
      }, 2000);
    } catch (e) {
      console.error('Failed to copy transcription:', e);
      await message('Failed to copy to clipboard', {
        title: 'Error',
        kind: 'error'
      });
    }
  }
 
  function getModelDisplayName(modelId: ModelId | null): string {
    if (!modelId) return 'Unknown';

    const engineNames: Record<ModelId['engine'], string> = {
      'whisper': 'Whisper',
      'moonshine': 'Moonshine',
      'parakeet-tdt': 'Parakeet',
    };

    return `${engineNames[modelId.engine]} ${modelId.id}`;
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

<Page>
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
                  Model: {getModelDisplayName(transcription.model_id)} •
                  Duration: {formatDuration(transcription.duration_ms)} •
                  Size: {formatSize(transcription.audio_size_bytes)}
                </Card.Description>
              </div>
              <div class="flex items-center gap-2">
                <Tooltip.Root>
                  <Tooltip.Trigger>
                    {#snippet child({ props })}
                      <Button.Root
                        {...props}
                        variant="destructive"
                        size="sm"
                        onclick={() => deleteTranscription(transcription.id)}
                        class="hover:opacity-80 hover:cursor-pointer"
                      >
                        <TrashIcon />
                      </Button.Root>
                    {/snippet}
                  </Tooltip.Trigger>
                  <Tooltip.Content>
                    <p>Delete transcription</p>
                  </Tooltip.Content>
                </Tooltip.Root>
                {#if copiedId === transcription.id}
                  <Tooltip.Root open>
                    <Tooltip.Trigger>
                      {#snippet child({ props })}
                        <Button.Root
                          {...props}
                          variant="ghost"
                          size="sm"
                          onclick={() => copyTranscription(transcription.id, transcription.text)}
                          class="hover:opacity-80 hover:cursor-pointer text-emerald-600"
                          aria-live="polite"
                        >
                          <CheckIcon />
                        </Button.Root>
                      {/snippet}
                    </Tooltip.Trigger>
                    <Tooltip.Content>
                      <p>Copied!</p>
                    </Tooltip.Content>
                  </Tooltip.Root>
                {:else}
                  <Tooltip.Root>
                    <Tooltip.Trigger>
                      {#snippet child({ props })}
                        <Button.Root
                          {...props}
                          variant="ghost"
                          size="sm"
                          onclick={() => copyTranscription(transcription.id, transcription.text)}
                          class="hover:opacity-80 hover:cursor-pointer"
                          aria-live="polite"
                        >
                          <CopyIcon />
                        </Button.Root>
                      {/snippet}
                    </Tooltip.Trigger>
                    <Tooltip.Content>
                      <p>Copy transcription</p>
                    </Tooltip.Content>
                  </Tooltip.Root>
                {/if}
              </div>
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
