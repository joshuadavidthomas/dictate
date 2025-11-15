/**
 * Shared type definitions for the Tauri backend API
 */

export type RecordingStatus = 'idle' | 'recording' | 'transcribing';

export type OutputMode = 'print' | 'copy' | 'insert';

export type OsdPosition = 'top' | 'bottom';

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
  is_default: boolean;
}

export type SampleRate = 16000 | 22050 | 44100 | 48000;

export interface SampleRateOption {
  value: SampleRate;
  label: string;
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
