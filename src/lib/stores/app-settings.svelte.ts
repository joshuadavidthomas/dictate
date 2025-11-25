/**
 * App Settings Store - Unified configuration management
 * Manages all persisted settings from ~/.config/dictate/config.toml
 */

import { createContext } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { settingsApi } from '$lib/api';
import { modelsApi } from '$lib/api';
import type { ShortcutCapabilities } from '$lib/api/settings';
import type { OutputMode, OsdPosition, AudioDevice, SampleRateOption, ModelId } from '$lib/api/types';

export interface InitialSettingsData {
  outputMode: OutputMode;
  windowDecorations: boolean;
  osdPosition: OsdPosition;
  currentDevice: string;
  availableDevices: string[];
  currentSampleRate: number;
  availableSampleRates: SampleRateOption[];
  preferredModel: ModelId | null;
  shortcut: string | null;
  shortcutCapabilities: ShortcutCapabilities | null;
}

export class AppSettingsState {
  // === GENERAL SETTINGS ===
  outputMode = $state<OutputMode>('print');
  windowDecorations = $state(true);
  osdPosition = $state<OsdPosition>('top');
  
  // === AUDIO SETTINGS ===
  currentDevice = $state<string>("default");
  availableDevices = $state<string[]>([]);
  isLoadingDevices = $state(false);
  
  currentSampleRate = $state<number>(16000);
  availableSampleRates = $state<SampleRateOption[]>([]);
  isLoadingSampleRates = $state(false);
  
  // === MODEL PREFERENCE ===
  preferredModel = $state<ModelId | null>(null);
  preferredModelValue = $state('');
  
  // === KEYBOARD SHORTCUT ===
  shortcut = $state<string | null>(null);
  shortcutCapabilities = $state<ShortcutCapabilities | null>(null);
  
  // === CONFIG FILE SYNC ===
  configChanged = $state(false);
  private checkingConfig = false;
  
  constructor(initialData?: InitialSettingsData) {
    if (initialData) {
      // Initialize with pre-loaded data
      this.outputMode = initialData.outputMode;
      this.windowDecorations = initialData.windowDecorations;
      this.osdPosition = initialData.osdPosition;
      this.currentDevice = initialData.currentDevice;
      this.availableDevices = initialData.availableDevices;
      this.currentSampleRate = initialData.currentSampleRate;
      this.availableSampleRates = initialData.availableSampleRates;
      this.preferredModel = initialData.preferredModel;
      this.preferredModelValue = initialData.preferredModel 
        ? `${initialData.preferredModel.engine}:${initialData.preferredModel.id}` 
        : '';
      this.shortcut = initialData.shortcut;
      this.shortcutCapabilities = initialData.shortcutCapabilities;
    } else {
      // Fallback: load data asynchronously
      this.init();
    }
  }
  
  private async init() {
    await this.loadAll();
  }
  
  /**
   * Load all settings from backend
   */
  async loadAll() {
    await Promise.all([
      this.loadGeneralSettings(),
      this.loadAudioSettings(),
      this.loadModelPreference(),
      this.loadShortcut(),
      this.loadShortcutCapabilities()
    ]);
  }

  
  /**
   * Load general app settings
   */
  private async loadGeneralSettings() {
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
      console.error('Failed to load general settings:', err);
      throw err;
    }
  }
  
  /**
   * Load audio settings
   */
  private async loadAudioSettings() {
    await this.loadDevices();
    await this.loadCurrentDevice();
    await this.loadSampleRates();
    await this.loadCurrentSampleRate();
  }
  
  /**
   * Load model preference
   */
  private async loadModelPreference() {
    try {
      const pref = await modelsApi.getPreferred();
      this.preferredModel = pref;
      this.preferredModelValue = pref ? `${pref.engine}:${pref.id}` : '';
    } catch (err) {
      console.error('Failed to load model preference:', err);
    }
  }
  
  // === GENERAL SETTINGS METHODS ===
  
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
  
  // === AUDIO SETTINGS METHODS ===
  
  async loadDevices() {
    this.isLoadingDevices = true;
    try {
      const devices = await invoke("list_audio_devices") as AudioDevice[];
      this.availableDevices = devices.map(d => d.name);
      await new Promise(resolve => setTimeout(resolve, 500));
    } catch (err) {
      console.error("Failed to load audio devices:", err);
    } finally {
      this.isLoadingDevices = false;
    }
  }

  async loadCurrentDevice() {
    try {
      const device = await invoke("get_audio_device") as string | null;
      this.currentDevice = device ?? "default";
    } catch (err) {
      console.error("Failed to load current device:", err);
    }
  }

  async setDevice(name: string) {
    try {
      // Convert "default" to null for backend
      const backendValue = name === "default" ? null : name;
      await invoke("set_audio_device", { deviceName: backendValue });
      this.currentDevice = name;
      this.configChanged = false;
      // Reload sample rates for the newly set device
      await this.loadSampleRates();
    } catch (err) {
      console.error("Failed to set audio device:", err);
      throw err;
    }
  }

  async loadSampleRates() {
    this.isLoadingSampleRates = true;
    try {
      const deviceName = this.currentDevice === "default" ? null : this.currentDevice;
      this.availableSampleRates = await invoke(
        "get_sample_rate_options_for_device",
        { deviceName }
      ) as SampleRateOption[];
    } catch (err) {
      console.error("Failed to load sample rate options:", err);
      // Fallback to all options
      try {
        this.availableSampleRates = await invoke("get_sample_rate_options") as SampleRateOption[];
      } catch (fallbackErr) {
        console.error("Failed to load fallback sample rate options:", fallbackErr);
      }
    } finally {
      this.isLoadingSampleRates = false;
    }
  }

  async loadCurrentSampleRate() {
    try {
      const rate = await invoke("get_sample_rate") as number;
      this.currentSampleRate = rate;
    } catch (err) {
      console.error("Failed to load current sample rate:", err);
    }
  }

  async setSampleRate(rate: number) {
    try {
      await invoke("set_sample_rate", { sampleRate: rate });
      this.currentSampleRate = rate;
      this.configChanged = false;
    } catch (err) {
      console.error("Failed to set sample rate:", err);
      throw err;
    }
  }
  
  // === MODEL PREFERENCE METHODS ===
  
  async setPreferredModel(value: string) {
    this.preferredModelValue = value;
    const id = value === '' ? null : this.stringToModelId(value);
    if (value !== '' && id === null) return;

    try {
      await modelsApi.setPreferred(id);
      this.preferredModel = id;
      this.configChanged = false;
    } catch (err) {
      console.error('Failed to set preferred model', err);
    }
  }
  
  private stringToModelId(value: string): ModelId | null {
    const [engine, id] = value.split(':');
    if (engine !== 'whisper' && engine !== 'parakeet') return null;
    return { engine, id } as ModelId;
  }
  
  // === KEYBOARD SHORTCUT METHODS ===
  
  /**
   * Load keyboard shortcut
   */
  private async loadShortcut() {
    try {
      const shortcut = await settingsApi.getShortcut();
      this.shortcut = shortcut;
    } catch (err) {
      console.error('Failed to load keyboard shortcut:', err);
    }
  }

  private async loadShortcutCapabilities() {
    try {
      this.shortcutCapabilities = await settingsApi.getShortcutCapabilities();
    } catch (err) {
      console.error('Failed to load shortcut capabilities:', err);
      this.shortcutCapabilities = null;
    }
  }
  
  /**
   * Set keyboard shortcut
   */
  async setShortcut(shortcut: string | null) {
    try {
      await settingsApi.setShortcut(shortcut);
      this.shortcut = shortcut;
      this.configChanged = false;
    } catch (err) {
      console.error('Failed to set keyboard shortcut:', err);
      throw err;
    }
  }
  
  /**
   * Validate a keyboard shortcut
   */
  async validateShortcut(shortcut: string): Promise<boolean> {
    try {
      return await settingsApi.validateShortcut(shortcut);
    } catch (err) {
      return false;
    }
  }
  
  // === CONFIG FILE SYNC METHODS ===
  
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
        const [fileMode, fileDecorations, filePosition, fileDevice, fileSampleRate, fileModel, fileShortcut] = await Promise.all([
          settingsApi.getOutputMode(),
          settingsApi.getWindowDecorations(),
          settingsApi.getOsdPosition(),
          invoke("get_audio_device") as Promise<string | null>,
          invoke("get_sample_rate") as Promise<number>,
          modelsApi.getPreferred(),
          settingsApi.getShortcut()
        ]);
        
        if (
          fileMode !== this.outputMode ||
          fileDecorations !== this.windowDecorations ||
          filePosition !== this.osdPosition ||
          (fileDevice ?? "default") !== this.currentDevice ||
          fileSampleRate !== this.currentSampleRate ||
          JSON.stringify(fileModel) !== JSON.stringify(this.preferredModel) ||
          fileShortcut !== this.shortcut
        ) {
          this.configChanged = true;
        } else {
          await settingsApi.markConfigSynced();
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
      await this.loadAll();
      await settingsApi.markConfigSynced();
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
        settingsApi.setOsdPosition(this.osdPosition),
        invoke("set_audio_device", { deviceName: this.currentDevice === "default" ? null : this.currentDevice }),
        invoke("set_sample_rate", { sampleRate: this.currentSampleRate }),
        modelsApi.setPreferred(this.preferredModel),
        settingsApi.setShortcut(this.shortcut)
      ]);
      this.configChanged = false;
    } catch (err) {
      console.error('Failed to save config:', err);
      throw err;
    }
  }
}

export const [getAppSettingsState, setAppSettingsState] = createContext<AppSettingsState>();

export const createAppSettingsState = (initialData?: InitialSettingsData) => {
  const settings = new AppSettingsState(initialData);
  setAppSettingsState(settings);
  return settings;
}
