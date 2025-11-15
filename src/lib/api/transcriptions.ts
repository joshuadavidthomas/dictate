/**
 * Transcriptions API - handles transcription history
 */

import { invoke } from '@tauri-apps/api/core';
import type { Transcription } from './types';

export const transcriptionsApi = {
  /**
   * Get transcription history with pagination
   */
  async list(limit = 100, offset = 0): Promise<Transcription[]> {
    return invoke('get_transcription_history', { limit, offset });
  },

  /**
   * Search transcription history
   */
  async search(query: string, limit = 100): Promise<Transcription[]> {
    return invoke('search_transcription_history', { query, limit });
  },

  /**
   * Get a single transcription by ID
   */
  async getById(id: number): Promise<Transcription | null> {
    return invoke('get_transcription_by_id', { id });
  },

  /**
   * Delete a transcription by ID
   */
  async delete(id: number): Promise<boolean> {
    return invoke('delete_transcription_by_id', { id });
  },

  /**
   * Get total count of transcriptions
   */
  async count(): Promise<number> {
    return invoke('get_transcription_count');
  }
};
