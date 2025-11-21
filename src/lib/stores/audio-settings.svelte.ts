import { invoke } from "@tauri-apps/api/core";
import { createContext } from 'svelte';
import type { AudioDevice, SampleRateOption } from '$lib/api/types';

export class AudioSettingsState {
  currentDevice = $state<string>("default");
  availableDevices = $state<string[]>([]);
  isLoadingDevices = $state(false);

  currentSampleRate = $state<number>(16000);
  availableSampleRates = $state<SampleRateOption[]>([]);
  isLoadingSampleRates = $state(false);

  constructor() {
    this.init();
  }

  private async init() {
    await this.loadDevices();
    await this.loadCurrentDevice();
    await this.loadSampleRates();
    await this.loadCurrentSampleRate();
  }

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
    } catch (err) {
      console.error("Failed to set sample rate:", err);
      throw err;
    }
  }
}

export const [getAudioSettingsState, setAudioSettingsState] = createContext<AudioSettingsState>();

export const createAudioSettingsState = () => {
  const audioSettings = new AudioSettingsState();
  setAudioSettingsState(audioSettings);
  return audioSettings;
}
