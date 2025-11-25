/**
 * Models API - manage transcription models and preferences
 */

import { invoke } from '@tauri-apps/api/core';
import type { ModelId, ModelInfo, ModelStorageInfo, ModelSize } from './types';

export const modelsApi = {
  /**
   * List all supported models with their download status.
   */
  async list(): Promise<ModelInfo[]> {
    return invoke('list_models');
  },

  /**
   * Get storage info for downloaded models.
   */
  async getStorageInfo(): Promise<ModelStorageInfo> {
    return invoke('get_model_storage_info');
  },

  /**
   * Get approximate sizes for all models.
   */
  async getSizes(): Promise<ModelSize[]> {
    return invoke('get_model_sizes');
  },

  /**
   * Download a specific model.
   */
  async download(id: ModelId): Promise<void> {
    return invoke('download_model', { id });
  },

  /**
   * Remove a downloaded model.
   */
  async remove(id: ModelId): Promise<void> {
    return invoke('remove_model', { id });
  },

  /**
   * Get the preferred model from settings.
   */
  async getPreferred(): Promise<ModelId | null> {
    return invoke('get_setting', { key: 'preferred_model' });
  },

  /**
   * Set the preferred model in settings.
   */
  async setPreferred(id: ModelId | null): Promise<void> {
    return invoke('set_setting', { key: 'preferred_model', value: id });
  },
};
