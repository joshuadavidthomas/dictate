/**
 * Transcriptions store - manages transcription history
 */

import { createContext } from 'svelte';
import { transcriptionsApi } from '$lib/api';
import type { Transcription } from '$lib/api/types';

export class TranscriptionsState {
  items = $state<Transcription[]>([]);
  totalCount = $state(0);
  loading = $state(false);
  error = $state('');
  searchQuery = $state('');
  
  constructor() {
    this.load();
  }

  /**
   * Load transcriptions (with optional search)
   */
  async load(limit = 100, offset = 0) {
    this.loading = true;
    this.error = '';
    
    try {
      if (this.searchQuery.trim()) {
        this.items = await transcriptionsApi.search(this.searchQuery, limit);
      } else {
        this.items = await transcriptionsApi.list(limit, offset);
      }
      
      this.totalCount = await transcriptionsApi.count();
    } catch (err) {
      this.error = `Failed to load transcriptions: ${err}`;
      console.error('Error loading transcriptions:', err);
      throw err;
    } finally {
      this.loading = false;
    }
  }
  
  /**
   * Set search query and reload
   */
  async search(query: string) {
    this.searchQuery = query;
    await this.load();
  }
  
  /**
   * Clear search and reload
   */
  async clearSearch() {
    this.searchQuery = '';
    await this.load();
  }
  
  /**
   * Delete a transcription
   */
  async delete(id: number) {
    try {
      const deleted = await transcriptionsApi.delete(id);
      if (deleted) {
        await this.load();
      }
      return deleted;
    } catch (err) {
      console.error('Error deleting transcription:', err);
      throw err;
    }
  }
  
  /**
   * Get a single transcription by ID
   */
  async getById(id: number) {
    try {
      return await transcriptionsApi.getById(id);
    } catch (err) {
      console.error('Error getting transcription:', err);
      throw err;
    }
  }
  
  /**
   * Refresh the current view
   */
  async refresh() {
    await this.load();
  }
}

export const [getTranscriptionsState, setTranscriptionsState] = createContext<TranscriptionsState>();

export const createTranscriptionsState = () => {
  const transcriptions = new TranscriptionsState();
  setTranscriptionsState(transcriptions);
  return transcriptions;
}
