/**
 * Shared type definitions for the Tauri backend API
 */

export type RecordingStatus = 'idle' | 'recording' | 'transcribing';

export type OutputMode = 'print' | 'copy' | 'insert';

export type OsdPosition = 'top' | 'bottom';

export type ModelEngine = 'whisper' | 'parakeet';

export type WhisperModel = 'tiny' | 'base' | 'small' | 'medium';

export type ParakeetModel = 'v2' | 'v3';

export type ModelId =
  | { engine: 'whisper'; id: WhisperModel }
  | { engine: 'parakeet'; id: ParakeetModel };

export interface ModelInfo {
  id: ModelId;
  engine: ModelEngine;
  is_downloaded: boolean;
  is_directory: boolean;
  download_url: string | null;
}

export interface ModelStorageInfo {
  models_dir: string;
  total_size_bytes: number;
  downloaded_count: number;
  available_count: number;
}

export interface ModelSize {
  id: ModelId;
  size_bytes: number;
}
 
export interface Transcription {
  id: number;
  text: string;
  created_at: number;
  duration_ms: number | null;
  model_name: string | null;
  audio_path: string | null;
  output_mode: string | null;
  audio_size_bytes: number | null;
}

export interface AudioDevice {
  name: string;
}

export type SampleRate = 16000 | 22050 | 44100 | 48000;

export interface SampleRateOption {
  value: number;
  is_recommended: boolean;
}

export interface AudioLevel {
  level: number;
}

export interface StatusUpdate {
  state: RecordingStatus;
}

export interface TranscriptionResult {
  text: string;
}
