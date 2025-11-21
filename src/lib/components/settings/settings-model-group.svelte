<script lang="ts">
  import { SettingsRadioGroup, SettingsRadioGroupItem } from "$lib/components/settings";
  import { Button } from "$lib/components/ui/button";
  import { Progress } from "$lib/components/ui/progress";
  import { getModelsState, modelIdToString, modelKey } from "$lib/stores/transcription-models.svelte";
  import DownloadIcon from "@lucide/svelte/icons/download";
  import Loader2Icon from "@lucide/svelte/icons/loader-2";
  import TrashIcon from "@lucide/svelte/icons/trash";

  type Props = {
    familyName: string;
  };

  let { familyName }: Props = $props();

  const modelsState = getModelsState();
  const filteredModels = $derived(
    familyName === "Parakeet" ? modelsState.parakeetModels : modelsState.whisperModels
  );
  const groupId = $derived(`transcription-model-${familyName.toLowerCase()}`);
</script>

{#if filteredModels.length}
  <SettingsRadioGroup
    id={groupId}
    label={familyName}
    bind:value={modelsState.preferredModelValue}
    onValueChange={modelsState.setPreferred}
  >
    {#snippet description()}
      {familyName} transcription models
    {/snippet}

    {#each filteredModels as model (modelKey(model.id))}
      <SettingsRadioGroupItem
        class="relative overflow-hidden"
        value={modelIdToString(model.id)}
        disabled={!model.is_downloaded}
      >
        <div class="flex w-full flex-col gap-1">
          <div class="flex w-full items-center justify-between gap-4">
            <div class="flex flex-col gap-1" class:text-muted-foreground={!model.is_downloaded}>
              <span class="font-medium">
                {familyName} {model.id.id}
                {#if modelsState.isActiveModel(model) && model.is_downloaded}
                  <span class="ml-2 rounded-full bg-green-100 px-2 py-0.5 text-xs font-medium text-green-700 dark:bg-green-900/40 dark:text-green-300">
                    Active
                  </span>
                {/if}
              </span>
              {#if modelsState.formatModelSize(model.id)}
                <span class="text-xs text-muted-foreground">
                  {modelsState.formatModelSize(model.id)}
                </span>
              {/if}
            </div>

            <div class="flex items-center gap-2">
              {#if model.is_downloaded}
                <Button
                  size="sm"
                  variant="destructive"
                  onclick={() => modelsState.remove(model)}
                  disabled={modelsState.removing[modelKey(model.id)]}
                >
                  <TrashIcon class="mr-1 h-3 w-3" />
                  Delete
                </Button>
              {:else}
                <Button
                  size="sm"
                  variant="ghost"
                  class="border border-transparent hover:border-border"
                  onclick={() => modelsState.download(model)}
                  disabled={modelsState.downloading[modelKey(model.id)]}
                >
                  {#if modelsState.downloading[modelKey(model.id)]}
                    <Loader2Icon class="mr-1 h-3 w-3 animate-spin" />
                    Downloading
                  {:else}
                    <DownloadIcon class="mr-1 h-3 w-3" />
                    Download
                  {/if}
                </Button>
              {/if}
            </div>
          </div>

          {#if modelsState.downloading[modelKey(model.id)] && modelsState.downloadProgress[modelKey(model.id)]}
            {@const p = modelsState.downloadProgress[modelKey(model.id)]}
            {@const percent =
              p.totalBytes > 0
                ? Math.round((p.downloadedBytes / p.totalBytes) * 100)
                : 0}

            <Progress value={percent} class="h-1 absolute inset-x-0 bottom-0 rounded-none" style="--primary: var(--color-emerald-500);" />
          {/if}
        </div>
      </SettingsRadioGroupItem>
    {/each}
  </SettingsRadioGroup>
{/if}
