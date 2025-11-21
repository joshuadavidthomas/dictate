import { createContext } from 'svelte';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { recordingApi } from '$lib/api';
import type { RecordingStatus, StatusUpdate, TranscriptionResult } from '$lib/api/types';

export class RecordingState {
  status = $state<RecordingStatus>('idle');
  transcriptionText = $state('');

  isRecording = $derived(this.status === 'recording');
  isTranscribing = $derived(this.status === 'transcribing');
  isIdle = $derived(this.status === 'idle');

  private unlisteners: UnlistenFn[] = [];

  constructor() {
    this.setupListeners();
  }

  private async setupListeners() {
    // Listen for recording events from Tauri backend
    this.unlisteners.push(
      await listen<StatusUpdate>('recording-started', () => {
        this.status = 'recording';
        this.transcriptionText = '';
      })
    );

    this.unlisteners.push(
      await listen<StatusUpdate>('recording-stopped', () => {
        this.status = 'transcribing';
      })
    );

    this.unlisteners.push(
      await listen<StatusUpdate>('transcription-complete', () => {
        this.status = 'idle';
      })
    );

    this.unlisteners.push(
      await listen<TranscriptionResult>('transcription-result', (event) => {
        this.transcriptionText = event.payload.text;
      })
    );
  }

  async toggle() {
    try {
      await recordingApi.toggle();
    } catch (err) {
      console.error('Toggle failed:', err);
      throw err;
    }
  }

  async loadStatus() {
    try {
      this.status = await recordingApi.getStatus();
    } catch (err) {
      console.error('Failed to load status:', err);
      throw err;
    }
  }

  destroy() {
    this.unlisteners.forEach(unlisten => unlisten());
    this.unlisteners = [];
  }
}

export const [getRecordingState, setRecordingState] = createContext<RecordingState>();

export const createRecordingState = () => {
  const recording = new RecordingState();
  setRecordingState(recording);
  return recording;
}
