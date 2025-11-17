/**
 * Settings API - handles app configuration
 */

import { invoke } from '@tauri-apps/api/core';
import type { OutputMode, OsdPosition } from './types';

export const settingsApi = {
  /**
   * Get current output mode
   */
  async getOutputMode(): Promise<OutputMode> {
    return invoke('get_output_mode');
  },

  /**
   * Set output mode
   */
  async setOutputMode(mode: OutputMode): Promise<void> {
    return invoke('set_output_mode', { mode });
  },

  /**
   * Get window decorations setting
   */
  async getWindowDecorations(): Promise<boolean> {
    return invoke('get_window_decorations');
  },

  /**
   * Set window decorations
   */
  async setWindowDecorations(enabled: boolean): Promise<void> {
    return invoke('set_window_decorations', { enabled });
  },

  /**
   * Get OSD position
   */
  async getOsdPosition(): Promise<OsdPosition> {
    return invoke('get_osd_position');
  },

  /**
   * Set OSD position
   */
  async setOsdPosition(position: OsdPosition): Promise<void> {
    return invoke('set_osd_position', { position });
  },

  /**
   * Check if config file has been modified externally
   */
  async checkConfigChanged(): Promise<boolean> {
    return invoke('check_config_changed');
  },

  /**
   * Mark the current config as synced with disk
   */
  async markConfigSynced(): Promise<void> {
    return invoke('mark_config_synced');
  }
};
