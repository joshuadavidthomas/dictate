/**
 * Settings store - manages app settings
 */

import { settingsApi } from '$lib/api';
import type { OutputMode, OsdPosition } from '$lib/api/types';

class SettingsStore {
  outputMode = $state<OutputMode>('print');
  windowDecorations = $state(true);
  osdPosition = $state<OsdPosition>('top');
  configChanged = $state(false);
  
  private checkingConfig = false;
  
  /**
   * Load all settings from backend
   */
  async load() {
    try {
      const [mode, decorations, position] = await Promise.all([
        settingsApi.getOutputMode(),
        settingsApi.getWindowDecorations(),
        settingsApi.getOsdPosition()
      ]);
      
      this.outputMode = mode;
      this.windowDecorations = decorations;
      this.osdPosition = position;
    } catch (err) {
      console.error('Failed to load settings:', err);
      throw err;
    }
  }
  
  /**
   * Set output mode
   */
  async setOutputMode(mode: OutputMode) {
    try {
      await settingsApi.setOutputMode(mode);
      this.outputMode = mode;
      this.configChanged = false;
    } catch (err) {
      console.error('Failed to set output mode:', err);
      throw err;
    }
  }
  
  /**
   * Set window decorations
   */
  async setWindowDecorations(enabled: boolean) {
    try {
      await settingsApi.setWindowDecorations(enabled);
      this.windowDecorations = enabled;
      this.configChanged = false;
    } catch (err) {
      console.error('Failed to set window decorations:', err);
      throw err;
    }
  }
  
  /**
   * Set OSD position
   */
  async setOsdPosition(position: OsdPosition) {
    try {
      await settingsApi.setOsdPosition(position);
      this.osdPosition = position;
      this.configChanged = false;
    } catch (err) {
      console.error('Failed to set OSD position:', err);
      throw err;
    }
  }
  
  /**
   * Check if config file has been modified externally
   */
  async checkConfigChanged() {
    if (this.checkingConfig) return;
    
    this.checkingConfig = true;
    try {
      const changed = await settingsApi.checkConfigChanged();
      
      if (changed) {
        // Reload from file to see if values differ
        const [fileMode, fileDecorations, filePosition] = await Promise.all([
          settingsApi.getOutputMode(),
          settingsApi.getWindowDecorations(),
          settingsApi.getOsdPosition()
        ]);
        
        if (
          fileMode !== this.outputMode ||
          fileDecorations !== this.windowDecorations ||
          filePosition !== this.osdPosition
        ) {
          this.configChanged = true;
        } else {
          await settingsApi.updateConfigMtime();
        }
      }
    } catch (err) {
      console.error('Failed to check config:', err);
    } finally {
      setTimeout(() => {
        this.checkingConfig = false;
      }, 1000);
    }
  }
  
  /**
   * Reload settings from config file
   */
  async reloadFromFile() {
    try {
      await this.load();
      await settingsApi.updateConfigMtime();
      this.configChanged = false;
    } catch (err) {
      console.error('Failed to reload config:', err);
      throw err;
    }
  }
  
  /**
   * Dismiss config changed banner (saves current UI values)
   */
  async dismissConfigChanged() {
    try {
      await Promise.all([
        settingsApi.setOutputMode(this.outputMode),
        settingsApi.setWindowDecorations(this.windowDecorations),
        settingsApi.setOsdPosition(this.osdPosition)
      ]);
      this.configChanged = false;
    } catch (err) {
      console.error('Failed to save config:', err);
      throw err;
    }
  }
}

export const settings = new SettingsStore();
