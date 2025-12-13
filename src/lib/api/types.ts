/**
 * Shared type definitions for the Tauri backend API
 */

export type RecordingStatus = 'idle' | 'recording' | 'transcribing';

export type OutputMode = 'print' | 'copy' | 'insert';

export type OsdPosition = 'top' | 'bottom';

export type Theme = 'light' | 'dark' | 'system';

export type ModelEngine = 'whisper' | 'moonshine' | 'parakeet-tdt';

export type WhisperModel = 'tiny-en' | 'tiny' | 'base-en' | 'base' | 'small-en' | 'small' | 'medium-en' | 'medium';

export type MoonshineModel = 'tiny-en' | 'base-en';

export type ParakeetTdtModel = 'v2' | 'v3';

export type ModelId =
  | { engine: 'whisper'; id: WhisperModel }
  | { engine: 'moonshine'; id: MoonshineModel }
  | { engine: 'parakeet-tdt'; id: ParakeetTdtModel };

export interface ModelInfo {
  engine: ModelEngine;
  id: WhisperModel | MoonshineModel | ParakeetTdtModel;
  is_downloaded: boolean;
  display_name: string;
}

export interface ModelStorageInfo {
  models_dir: string;
  total_size_bytes: number;
  downloaded_count: number;
  available_count: number;
}

export interface ModelSize {
  engine: ModelEngine;
  id: WhisperModel | MoonshineModel | ParakeetTdtModel;
  size_bytes: number;
}

export interface Transcription {
  id: number;
  text: string;
  created_at: number;
  duration_ms: number | null;
  model_id: ModelId | null;
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
