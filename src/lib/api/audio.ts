/**
 * Audio API - handles audio device configuration
 */

import { invoke } from '@tauri-apps/api/core';
import type { AudioDevice, SampleRate, SampleRateOption, AudioLevel } from './types';

export const audioApi = {
  /**
   * List available audio input devices
   */
  async listDevices(): Promise<AudioDevice[]> {
    return invoke('list_audio_devices');
  },

  /**
   * Get current audio device
   */
  async getDevice(): Promise<string | null> {
    return invoke('get_setting', { key: 'audio_device' });
  },

  /**
   * Set audio device
   */
  async setDevice(deviceName: string | null): Promise<void> {
    return invoke('set_setting', { key: 'audio_device', value: deviceName });
  },

  /**
   * Get current sample rate
   */
  async getSampleRate(): Promise<SampleRate> {
    return invoke('get_setting', { key: 'sample_rate' });
  },

  /**
   * Get available sample rate options
   */
  async getSampleRateOptions(): Promise<SampleRateOption[]> {
    return invoke('get_sample_rate_options');
  },

  /**
   * Set sample rate
   */
  async setSampleRate(sampleRate: SampleRate): Promise<void> {
    return invoke('set_setting', { key: 'sample_rate', value: sampleRate });
  },

  /**
   * Test audio device
   */
  async testDevice(): Promise<boolean> {
    return invoke('test_audio_device');
  },

  /**
   * Get current audio level
   */
  async getAudioLevel(): Promise<AudioLevel> {
    return invoke('get_audio_level');
  }
};
