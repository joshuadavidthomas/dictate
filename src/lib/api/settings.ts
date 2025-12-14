/**
 * Settings API - handles app configuration
 */

import { invoke } from '@tauri-apps/api/core';
import type { OutputMode, OsdPosition, Theme } from './types';

export type ShortcutCapabilities = {
  platform: string;
  canRegister: boolean;
  compositor: string | null;
};

export const settingsApi = {
  /**
   * Get current output mode
   */
  async getOutputMode(): Promise<OutputMode> {
    return invoke('get_setting', { key: 'output_mode' });
  },

  /**
   * Set output mode
   */
  async setOutputMode(mode: OutputMode): Promise<void> {
    return invoke('set_setting', { key: 'output_mode', value: mode });
  },

  /**
   * Get window decorations setting
   */
  async getWindowDecorations(): Promise<boolean> {
    return invoke('get_setting', { key: 'window_decorations' });
  },

  /**
   * Set window decorations
   */
  async setWindowDecorations(enabled: boolean): Promise<void> {
    return invoke('set_setting', { key: 'window_decorations', value: enabled });
  },

  /**
   * Get OSD position
   */
  async getOsdPosition(): Promise<OsdPosition> {
    return invoke('get_setting', { key: 'osd_position' });
  },

  /**
   * Set OSD position
   */
  async setOsdPosition(position: OsdPosition): Promise<void> {
    return invoke('set_setting', { key: 'osd_position', value: position });
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
  },

  /**
   * Get current keyboard shortcut
   */
  async getShortcut(): Promise<string | null> {
    return invoke('get_setting', { key: 'shortcut' });
  },

  /**
   * Set keyboard shortcut
   */
  async setShortcut(shortcut: string | null): Promise<void> {
    return invoke('set_setting', { key: 'shortcut', value: shortcut });
  },

  /**
   * Validate a keyboard shortcut
   */
  async validateShortcut(shortcut: string): Promise<boolean> {
    return invoke('validate_shortcut', { shortcut });
  },

  /**
   * Fetch shortcut backend capabilities (platform hints)
   */
  async getShortcutCapabilities(): Promise<ShortcutCapabilities> {
    return invoke('get_shortcut_capabilities');
  },

  /**
   * Get current theme preference
   */
  async getTheme(): Promise<Theme> {
    return invoke('get_setting', { key: 'theme' });
  },

  /**
   * Set theme preference
   */
  async setTheme(theme: Theme): Promise<void> {
    return invoke('set_setting', { key: 'theme', value: theme });
  }
};
