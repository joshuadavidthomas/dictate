//! Audio recording functionality
//!
//! Provides high-level audio recording using CPAL (Cross-Platform Audio Library).
//! Supports device enumeration, WAV file output, and real-time spectrum analysis.

use super::detection::SilenceDetector;
use super::spectrum::SpectrumAnalyzer;
use anyhow::{Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, StreamConfig};
use hound::{WavSpec, WavWriter};
use serde::{Serialize, Deserialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Audio recording device with configuration
pub struct AudioRecorder {
    device: Device,
    config: StreamConfig,
    sample_rate: u32,
}

/// Information about an available audio input device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub supported_sample_rates: Vec<u32>,
}

/// Sample rate option with metadata for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleRateOption {
    pub value: u32,
    pub label: String,
    pub description: String,
    pub is_recommended: bool,
}

/// Supported sample rates for audio recording
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleRate {
    Rate8kHz = 8000,
    Rate16kHz = 16000,
    Rate22kHz = 22050,
    Rate44kHz = 44100,
    Rate48kHz = 48000,
}

impl SampleRate {
    /// All available sample rates
    pub const ALL: [Self; 5] = [
        Self::Rate8kHz,
        Self::Rate16kHz,
        Self::Rate22kHz,
        Self::Rate44kHz,
        Self::Rate48kHz,
    ];
    
    /// Get all available sample rate options with UI metadata
    pub fn all_options() -> Vec<SampleRateOption> {
        Self::ALL.iter().map(|rate| rate.as_option()).collect()
    }
    
    /// Convert this sample rate to a SampleRateOption with metadata
    pub fn as_option(self) -> SampleRateOption {
        SampleRateOption {
            value: self.as_u32(),
            label: self.label().to_string(),
            description: self.description().to_string(),
            is_recommended: self.is_recommended(),
        }
    }
    
    /// Convert sample rate to u32 value
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
    
    /// Get human-readable label
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rate8kHz => "8 kHz",
            Self::Rate16kHz => "16 kHz",
            Self::Rate22kHz => "22 kHz",
            Self::Rate44kHz => "44.1 kHz",
            Self::Rate48kHz => "48 kHz",
        }
    }
    
    /// Get description for UI
    pub const fn description(self) -> &'static str {
        match self {
            Self::Rate8kHz => "Low quality",
            Self::Rate16kHz => "Recommended for speech",
            Self::Rate22kHz => "Standard quality",
            Self::Rate44kHz => "CD quality",
            Self::Rate48kHz => "Professional",
        }
    }
    
    /// Whether this is the recommended rate
    pub const fn is_recommended(self) -> bool {
        matches!(self, Self::Rate16kHz)
    }
}

impl std::convert::TryFrom<u32> for SampleRate {
    type Error = anyhow::Error;
    
    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            8000 => Ok(Self::Rate8kHz),
            16000 => Ok(Self::Rate16kHz),
            22050 => Ok(Self::Rate22kHz),
            44100 => Ok(Self::Rate44kHz),
            48000 => Ok(Self::Rate48kHz),
            _ => Err(anyhow!(
                "Unsupported sample rate: {}. Supported rates: {:?}",
                value,
                Self::ALL.iter().map(|r| r.as_u32()).collect::<Vec<_>>()
            )),
        }
    }
}

impl From<SampleRate> for u32 {
    fn from(rate: SampleRate) -> Self {
        rate.as_u32()
    }
}

impl AudioRecorder {
    /// Create a new audio recorder with optimal settings for speech (16kHz)
    pub fn new() -> Result<Self> {
        Self::new_with_device(None, 16000)
    }

    /// Create a new audio recorder with a specific device and sample rate
    ///
    /// # Arguments
    /// * `device_name` - Optional device name. If None, uses system default.
    /// * `sample_rate` - Target sample rate in Hz (e.g., 16000, 44100, 48000)
    pub fn new_with_device(device_name: Option<&str>, sample_rate: u32) -> Result<Self> {
        let host = cpal::default_host();
        
        let device = if let Some(name) = device_name {
            // Find device by name
            host.input_devices()?
                .find(|d| d.name().map(|n| n == name).unwrap_or(false))
                .ok_or_else(|| anyhow!("Audio device '{}' not found", name))?
        } else {
            // Use default device
            host.default_input_device()
                .ok_or_else(|| anyhow!("No default input device found"))?
        };

        let config = Self::get_optimal_config(&device, sample_rate)?;

        Ok(Self {
            device,
            config,
            sample_rate,
        })
    }

    /// Find the best audio configuration for the target sample rate
    fn get_optimal_config(device: &Device, target_sample_rate: u32) -> Result<StreamConfig> {
        let supported_configs = device.supported_input_configs()?;

        // Find config closest to target sample rate
        let mut best_config = None;
        let mut best_diff = u32::MAX;

        for config in supported_configs {
            let diff = (config.max_sample_rate().0).abs_diff(target_sample_rate);
            if diff < best_diff {
                best_diff = diff;
                best_config = Some(config);
            }
        }

        let config = best_config
            .ok_or_else(|| anyhow!("No suitable audio configuration found".to_string()))?;

        // Convert to 16kHz mono if needed
        let config = config.with_sample_rate(cpal::SampleRate(target_sample_rate));
        Ok(config.into())
    }

    /// List all available audio input devices
    pub fn list_devices() -> Result<Vec<AudioDeviceInfo>> {
        let host = cpal::default_host();
        let devices = host.input_devices()?;
        let default_device = host.default_input_device();

        let mut device_infos = Vec::new();

        for device in devices {
            let name = device.name().unwrap_or("Unknown Device".to_string());
            let is_default = default_device
                .as_ref()
                .map(|d| d.name().unwrap_or_default() == name)
                .unwrap_or(false);

            let supported_sample_rates = device
                .supported_input_configs()?
                .map(|c| c.max_sample_rate().0)
                .collect();

            device_infos.push(AudioDeviceInfo {
                name,
                is_default,
                supported_sample_rates,
            });
        }

        Ok(device_infos)
    }

    /// Get the name of the current device
    pub fn device_name(&self) -> Option<String> {
        self.device.name().ok()
    }

    /// Get the current sample rate
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Record a short audio sample and return the average volume level (0.0 to 1.0)
    pub fn get_audio_level(&self) -> Result<f32> {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let stop_signal = Arc::new(AtomicBool::new(false));
        
        let stream = self.start_recording_background(
            buffer.clone(),
            stop_signal.clone(),
            None,
            None,
        )?;
        
        stream.play()?;
        
        // Record for 100ms
        std::thread::sleep(std::time::Duration::from_millis(100));
        stop_signal.store(true, Ordering::Release);
        
        // Give it time to stop
        std::thread::sleep(std::time::Duration::from_millis(10));
        drop(stream);
        
        // Calculate RMS (root mean square) of the audio samples
        let samples = buffer.lock().unwrap();
        if samples.is_empty() {
            return Ok(0.0);
        }
        
        let sum_of_squares: f64 = samples.iter()
            .map(|&s| {
                let normalized = s as f64 / i16::MAX as f64;
                normalized * normalized
            })
            .sum();
        
        let rms = (sum_of_squares / samples.len() as f64).sqrt();
        Ok(rms as f32)
    }

    /// Start recording in background to a shared buffer (non-blocking)
    ///
    /// Optionally sends spectrum analysis updates via spectrum_tx channel.
    /// Recording can be stopped by setting stop_signal or via silence detection.
    pub fn start_recording_background(
        &self,
        audio_buffer: Arc<Mutex<Vec<i16>>>,
        stop_signal: Arc<AtomicBool>,
        silence_detector: Option<SilenceDetector>,
        spectrum_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<f32>>>,
    ) -> Result<cpal::Stream> {
        let buffer_clone = audio_buffer.clone();
        let stop_clone = stop_signal.clone();
        let silence_detector_clone = silence_detector.clone();

        // Create spectrum analyzer if we have a channel to send to
        let mut spectrum_analyzer = spectrum_tx
            .as_ref()
            .map(|_| SpectrumAnalyzer::new(self.sample_rate));

        let stream = self.device.build_input_stream(
            &self.config.clone(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if stop_clone.load(Ordering::Acquire) {
                    return;
                }

                // Check for silence detection
                if let Some(ref detector) = silence_detector_clone {
                    let has_sound = data.iter().any(|&sample| !detector.is_silent(sample));

                    if has_sound {
                        detector.update_sound_time();
                    } else if detector.should_stop() {
                        // Signal stop on silence
                        stop_clone.store(true, Ordering::Release);
                        return;
                    }
                }

                if let Ok(mut buffer) = buffer_clone.lock() {
                    for &sample in data {
                        let sample_i16 = (sample * i16::MAX as f32) as i16;
                        buffer.push(sample_i16);

                        // Calculate and send spectrum if analyzer exists
                        if let Some(ref mut analyzer) = spectrum_analyzer
                            && let Some(bands) = analyzer.push_sample(sample)
                            && let Some(ref tx) = spectrum_tx
                        {
                            let _ = tx.send(bands);
                        }
                    }
                }
            },
            |err| {
                eprintln!("Recording error: {}", err);
            },
            None,
        )?;

        Ok(stream)
    }
}

/// Convert audio buffer to WAV file
///
/// Writes a raw i16 audio buffer to a WAV file with the specified sample rate.
/// The output is always mono (single channel), 16-bit PCM.
///
/// # Arguments
/// * `buffer` - Raw audio samples as signed 16-bit integers
/// * `output_path` - Path where the WAV file should be written
/// * `sample_rate` - Sample rate in Hz (e.g., 16000 for 16kHz)
///
/// # Example
/// ```no_run
/// use dictate::audio::buffer_to_wav;
///
/// let samples: Vec<i16> = vec![0; 16000]; // 1 second of silence at 16kHz
/// buffer_to_wav(&samples, "output.wav", 16000).unwrap();
/// ```
pub fn buffer_to_wav<P: AsRef<Path>>(
    buffer: &[i16],
    output_path: P,
    sample_rate: u32,
) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(output_path, spec)?;
    for &sample in buffer {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(())
}
