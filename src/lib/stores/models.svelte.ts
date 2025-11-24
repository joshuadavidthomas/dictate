/**
 * Models Store - Model catalog and download management
 * Handles model availability, downloads, and operations (not preferences)
 */

import { createContext } from 'svelte';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { modelsApi } from '$lib/api';
import type { ModelId, ModelInfo } from '$lib/api/types';

// Helper functions
export function modelKey(id: ModelId): string {
  return `${id.engine}:${id.id}`;
}

export function modelIdToString(id: ModelId): string {
  return modelKey(id);
}

export function stringToModelId(value: string): ModelId | null {
  const [engine, id] = value.split(':');
  if (engine !== 'whisper' && engine !== 'parakeet') return null;
  return { engine, id } as ModelId;
}

type DownloadPhase = 'idle' | 'downloading' | 'extracting' | 'done' | 'error';

type DownloadProgressPayload = {
  id: ModelId;
  downloaded_bytes: number;
  total_bytes: number;
  phase: string;
};

type DownloadProgressData = {
  downloadedBytes: number;
  totalBytes: number;
  phase: DownloadPhase;
};

export class ModelsState {
  // Model catalog
  models = $state<ModelInfo[]>([]);
  modelSizes = $state.raw<Record<string, number>>({});
  
  // Operations state
  downloading = $state.raw<Record<string, boolean>>({});
  removing = $state.raw<Record<string, boolean>>({});
  
  // Per-model progress for status bar / UI
  downloadProgress = $state.raw<Record<string, DownloadProgressData>>({});
  
  // Loading/error state
  loading = $state(false);
  error = $state<string | null>(null);
  
  // Simple buffer: collect events, flush periodically
  private progressBuffer = new Map<string, DownloadProgressPayload>();
  private flushTimer: number | null = null;
  private readonly BUFFER_MS = 100; // Update UI max 10 times per second

  private unlisteners: UnlistenFn[] = [];

  // Derived - model lists grouped by engine
  parakeetModels = $derived(
    this.models
      .filter((m) => m.engine === 'parakeet')
      .slice()
      .sort((a, b) => {
        const order: Record<string, number> = { v3: 0, v2: 1 };
        return (order[a.id.id] ?? 99) - (order[b.id.id] ?? 99);
      })
  );

  whisperModels = $derived(
    this.models
      .filter((m) => m.engine === 'whisper')
      .slice()
      .sort((a, b) => {
        const sizeA = this.modelSizes[modelKey(a.id)] ?? Infinity;
        const sizeB = this.modelSizes[modelKey(b.id)] ?? Infinity;
        return sizeA - sizeB;
      })
  );

  // Derived helpers for UI logic
  initialLoading = $derived(this.loading && this.models.length === 0);

  hasAnyModels = $derived(
    this.parakeetModels.length > 0 || this.whisperModels.length > 0
  );

  hasError = $derived(this.error !== null);

  isAnyDownloading = $derived(
    Object.values(this.downloadProgress).some(
      (p) => p.phase === 'downloading' || p.phase === 'extracting'
    )
  );

  constructor(initialModels?: ModelInfo[], initialSizes?: Record<string, number>) {
    this.setupListeners();
    
    if (initialModels && initialSizes) {
      // Initialize with pre-loaded data
      this.models = initialModels;
      this.modelSizes = initialSizes;
      this.loading = false;
    } else {
      // Fallback: load data asynchronously
      this.refresh();
    }
  }

  private async setupListeners() {
    try {
      this.unlisteners.push(
        await listen<DownloadProgressPayload>('model-download-progress', (event) => {
          this.updateDownloadProgress(event.payload);
        })
      );
    } catch (err) {
      console.error('Failed to listen for model-download-progress', err);
    }
  }

  formatModelSize = (id: ModelId): string => {
    const key = modelKey(id);
    const bytes = this.modelSizes[key];
    if (!bytes || bytes <= 0) return '';
    const mb = bytes / 1_000_000;
    if (mb < 1) return `${(bytes / 1_000).toFixed(0)} KB`;
    return `${mb.toFixed(1)} MB`;
  };

  /**
   * Check if a model is the currently active/preferred model
   * @param model The model to check
   * @param preferredModel The currently preferred model (from AppSettingsState)
   */
  isActiveModel = (model: ModelInfo, preferredModel: ModelId | null): boolean => {
    if (!preferredModel) return false;
    return modelKey(model.id) === modelKey(preferredModel);
  };

  /**
   * Refresh model list from backend
   */
  refresh = async () => {
    this.loading = true;
    this.error = null;
    try {
      const [list, sizes] = await Promise.all([
        modelsApi.list(),
        modelsApi.getSizes(),
      ]);

      this.models = list;

      const nextSizes: Record<string, number> = {};
      for (const s of sizes) {
        nextSizes[modelKey(s.id)] = s.size_bytes;
      }
      this.modelSizes = nextSizes;
    } catch (err) {
      console.error('Failed to load models', err);
      this.error = 'Failed to load models';
    } finally {
      this.loading = false;
    }
  };

  /**
   * Download a model
   */
  download = async (model: ModelInfo) => {
    const key = modelKey(model.id);
    this.downloading = { ...this.downloading, [key]: true };
    this.error = null;

    try {
      await modelsApi.download(model.id);

      this.models = this.models.map((m) =>
        modelKey(m.id) === key ? { ...m, is_downloaded: true } : m
      );
    } catch (err) {
      console.error('Failed to download model', err);
      this.error = 'Failed to download model';
    } finally {
      this.downloading = { ...this.downloading, [key]: false };
    }
  };

  /**
   * Remove a model
   */
  remove = async (model: ModelInfo) => {
    const key = modelKey(model.id);
    this.removing = { ...this.removing, [key]: true };
    this.error = null;

    try {
      await modelsApi.remove(model.id);

      // Keep the model in the list but mark it as not downloaded
      this.models = this.models.map((m) =>
        modelKey(m.id) === key ? { ...m, is_downloaded: false } : m
      );
    } catch (err) {
      console.error('Failed to remove model', err);
      this.error = 'Failed to remove model';
    } finally {
      this.removing = { ...this.removing, [key]: false };
    }
  };

  /**
   * Update download progress from backend events
   * Buffers rapid updates and flushes to UI every 200ms
   */
  updateDownloadProgress = (payload: DownloadProgressPayload) => {
    const key = modelKey(payload.id);
    const phase = (payload.phase || 'downloading') as DownloadPhase;

    // Immediate handling for done/error
    if (phase === 'done' || phase === 'error') {
      this.progressBuffer.delete(key);
      const { [key]: _removed, ...rest } = this.downloadProgress;
      this.downloadProgress = rest;
      this.clearFlushTimer();
      return;
    }

    // Buffer the latest value for this model
    this.progressBuffer.set(key, payload);
    
    // Schedule flush if not already scheduled
    if (this.flushTimer === null) {
      this.flushTimer = window.setTimeout(() => this.flushProgressBuffer(), this.BUFFER_MS);
    }
  };
  
  /**
   * Flush buffered progress updates to state
   * Called every BUFFER_MS (200ms) while downloads are active
   */
  private flushProgressBuffer() {
    this.flushTimer = null;
    
    if (this.progressBuffer.size === 0) return;
    
    // Take all buffered values, update state once
    const updates: Record<string, DownloadProgressData> = {};
    
    for (const [key, payload] of this.progressBuffer.entries()) {
      updates[key] = {
        downloadedBytes: payload.downloaded_bytes,
        totalBytes: payload.total_bytes,
        phase: (payload.phase || 'downloading') as DownloadPhase,
      };
    }
    
    this.downloadProgress = {
      ...this.downloadProgress,
      ...updates,
    };
    
    this.progressBuffer.clear();
    
    // Schedule next flush if there are still active downloads
    if (Object.keys(this.downloadProgress).length > 0) {
      this.flushTimer = window.setTimeout(() => this.flushProgressBuffer(), this.BUFFER_MS);
    }
  }
  
  /**
   * Clear the flush timer
   */
  private clearFlushTimer() {
    if (this.flushTimer !== null) {
      clearTimeout(this.flushTimer);
      this.flushTimer = null;
    }
  }

  destroy() {
    this.unlisteners.forEach(unlisten => unlisten());
    this.unlisteners = [];
    this.clearFlushTimer();
  }
}

export const [getModelsState, setModelsState] = createContext<ModelsState>();

export const createModelsState = (initialModels?: ModelInfo[], initialSizes?: Record<string, number>) => {
  const models = new ModelsState(initialModels, initialSizes);
  setModelsState(models);
  return models;
}
