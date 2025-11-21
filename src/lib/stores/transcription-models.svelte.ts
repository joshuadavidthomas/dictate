import { createContext } from 'svelte';
import { modelsApi } from '$lib/api';
import type { ModelId, ModelInfo } from '$lib/api/types';

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

export class TranscriptionModelsState {
  models = $state<ModelInfo[]>([]);
  preferredModel: ModelId | null = $state<ModelId | null>(null);
  preferredModelValue = $state('');
  loading = $state(false);
  error = $state<string | null>(null);
  modelSizes = $state.raw<Record<string, number>>({});
  downloading = $state.raw<Record<string, boolean>>({});
  removing = $state.raw<Record<string, boolean>>({});

  // Per-model progress for status bar / UI
  downloadProgress = $state.raw<
    Record<
      string,
      {
        downloadedBytes: number;
        totalBytes: number;
        phase: DownloadPhase;
      }
    >>({});

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

  selectedModel = $derived(
    this.preferredModel
      ? this.models.find(
          (m) => modelKey(m.id) === modelKey(this.preferredModel as ModelId)
        ) ?? null
      : null
  );

  selectedModelLabel = $derived.by(() => {
    if (!this.selectedModel) return 'No model selected';

    const { id, engine, is_downloaded } = this.selectedModel;

    const baseName =
      engine === 'parakeet'
        ? `Parakeet ${id.id}`
        : engine === 'whisper'
        ? `Whisper ${id.id}`
        : id.id;

    const size = this.formatModelSize(id);
    const suffix = is_downloaded ? '' : ' (not downloaded)';

    return size ? `${baseName} • ${size}${suffix}` : `${baseName}${suffix}`;
  });

  isAnyDownloading = $derived(
    Object.values(this.downloadProgress).some(
      (p) => p.phase === 'downloading' || p.phase === 'extracting'
    )
  );

  constructor() {
    this.refresh();
  }

  formatModelSize = (id: ModelId): string => {
    const key = modelKey(id);
    const bytes = this.modelSizes[key];
    if (!bytes || bytes <= 0) return '';
    const mb = bytes / 1_000_000;
    if (mb < 1) return `${(bytes / 1_000).toFixed(0)} KB`;
    return `${mb.toFixed(1)} MB`;
  };

  isActiveModel = (model: ModelInfo): boolean => {
    if (!this.preferredModel) return false;
    return modelKey(model.id) === modelKey(this.preferredModel);
  };

  refresh = async () => {
    this.loading = true;
    this.error = null;
    try {
      const [list, pref, sizes] = await Promise.all([
        modelsApi.list(),
        modelsApi.getPreferred(),
        modelsApi.getSizes(),
      ]);

      this.models = list;
      this.preferredModel = pref;
      this.preferredModelValue = pref ? modelIdToString(pref) : '';

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

  setPreferred = async (value: string) => {
    this.preferredModelValue = value;
    const id = value === '' ? null : stringToModelId(value);
    if (value !== '' && id === null) return;

    try {
      await modelsApi.setPreferred(id);
      this.preferredModel = id;
    } catch (err) {
      console.error('Failed to set preferred model', err);
      this.error = 'Failed to set preferred model';
    }
  };

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

      if (this.preferredModel && modelKey(this.preferredModel) === key) {
        this.preferredModel = null;
        this.preferredModelValue = '';
      }
    } catch (err) {
      console.error('Failed to remove model', err);
      this.error = 'Failed to remove model';
    } finally {
      this.removing = { ...this.removing, [key]: false };
    }
  };

  updateDownloadProgress = (payload: {
    id: ModelId;
    downloaded_bytes: number;
    total_bytes: number;
    phase: string;
  }) => {
    const key = modelKey(payload.id);
    const phase = (payload.phase || 'downloading') as DownloadPhase;

    // When a download finishes or errors, clear progress so the bar disappears
    if (phase === 'done' || phase === 'error') {
      const { [key]: _removed, ...rest } = this.downloadProgress;
      this.downloadProgress = rest;
      return;
    }

    this.downloadProgress = {
      ...this.downloadProgress,
      [key]: {
        downloadedBytes: payload.downloaded_bytes,
        totalBytes: payload.total_bytes,
        phase,
      },
    };
  };
}

export const [getModelsState, setModelsState] = createContext<TranscriptionModelsState>();

export const createTranscriptionModelsState = () => {
  const models = new TranscriptionModelsState();
  setModelsState(models);
  return models;
}
