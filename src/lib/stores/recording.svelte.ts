/**
 * Recording store - manages recording state and transcription results
 */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { recordingApi } from '$lib/api';
import type { RecordingStatus, StatusUpdate, TranscriptionResult } from '$lib/api/types';

class RecordingStore {
  status = $state<RecordingStatus>('idle');
  transcriptionText = $state('');
  
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
  
  /**
   * Toggle recording on/off
   */
  async toggle() {
    try {
      await recordingApi.toggle();
    } catch (err) {
      console.error('Toggle failed:', err);
      throw err;
    }
  }
  
  /**
   * Load current recording status from backend
   */
  async loadStatus() {
    try {
      this.status = await recordingApi.getStatus();
    } catch (err) {
      console.error('Failed to load status:', err);
      throw err;
    }
  }
  
  /**
   * Check if currently recording
   */
  get isRecording() {
    return this.status === 'recording';
  }
  
  /**
   * Check if currently transcribing
   */
  get isTranscribing() {
    return this.status === 'transcribing';
  }
  
  /**
   * Check if idle (not recording or transcribing)
   */
  get isIdle() {
    return this.status === 'idle';
  }
  
  /**
   * Cleanup listeners (call when store is destroyed)
   */
  destroy() {
    this.unlisteners.forEach(unlisten => unlisten());
    this.unlisteners = [];
  }
}

export const recording = new RecordingStore();
