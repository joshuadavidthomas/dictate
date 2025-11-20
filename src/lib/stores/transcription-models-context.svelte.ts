import { createContext } from 'svelte';
import type { TranscriptionModelsState } from '$lib/stores/transcription-models.svelte';

export const [getModelsState, setModelsState] =
  createContext<TranscriptionModelsState>();
