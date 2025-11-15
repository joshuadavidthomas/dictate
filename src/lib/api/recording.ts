/**
 * Recording API - handles recording control and status
 */

import { invoke } from '@tauri-apps/api/core';
import type { RecordingStatus } from './types';

export const recordingApi = {
  /**
   * Toggle recording on/off
   */
  async toggle(): Promise<string> {
    return invoke('toggle_recording');
  },

  /**
   * Get current recording status
   */
  async getStatus(): Promise<RecordingStatus> {
    return invoke('get_status');
  },

  /**
   * Get app version
   */
  async getVersion(): Promise<string> {
    return invoke('get_version');
  }
};
