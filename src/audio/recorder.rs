//! Audio recording functionality
//!
//! Provides high-level audio recording using CPAL (Cross-Platform Audio Library).
//! Supports device enumeration, WAV file output, and real-time spectrum analysis.

use super::detection::SilenceDetector;
use super::spectrum::SpectrumAnalyzer;
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use hound::{WavSpec, WavWriter};
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Audio recording device with configuration
pub struct AudioRecorder {
    device: Device,
    config: StreamConfig,
    sample_rate: u32,
}

/// Information about an available audio input device
#[derive(Debug)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub supported_sample_rates: Vec<u32>,
    pub supported_formats: Vec<SampleFormat>,
}

impl AudioRecorder {
    /// Create a new audio recorder with optimal settings for speech (16kHz)
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("No default input device found"))?;

        let sample_rate = 16000; // Whisper optimal sample rate
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
            let diff = (config.max_sample_rate().0 as u32).abs_diff(target_sample_rate);
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
                .map(|c| c.max_sample_rate().0 as u32)
                .collect();

            let supported_formats = device
                .supported_input_configs()?
                .map(|c| c.sample_format())
                .collect();

            device_infos.push(AudioDeviceInfo {
                name,
                is_default,
                supported_sample_rates,
                supported_formats,
            });
        }

        Ok(device_infos)
    }

    /// Record audio to a WAV file (blocking)
    ///
    /// Records until max_duration is reached or silence is detected.
    /// Returns the actual recording duration.
    pub fn record_to_wav<P: AsRef<Path>>(
        &self,
        output_path: P,
        max_duration: Duration,
        silence_detector: Option<SilenceDetector>,
    ) -> Result<Duration> {
        let start_time = Instant::now();
        let spec = WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let file = File::create(output_path)?;
        let writer = WavWriter::new(file, spec)?;
        let writer = Arc::new(Mutex::new(writer));
        let writer_clone = writer.clone();

        let silence_detector_clone = silence_detector.clone();
        let should_stop = Arc::new(Mutex::new(false));
        let should_stop_clone = should_stop.clone();

        let stream =
            self.build_stream::<i16>(writer_clone, silence_detector_clone, should_stop_clone)?;

        stream.play()?;

        // Record until max duration or silence detected
        loop {
            let elapsed = start_time.elapsed();
            if elapsed >= max_duration {
                break;
            }

            if let Some(ref detector) = silence_detector {
                if detector.should_stop() && elapsed > Duration::from_secs(1) {
                    println!("Silence detected, stopping recording");
                    break;
                }
            }

            if let Ok(should_stop) = should_stop.lock() {
                if *should_stop {
                    break;
                }
            }

            std::thread::sleep(Duration::from_millis(100));
        }

        drop(stream);

        // Finalize the WAV file
        if let Ok(writer) = Arc::try_unwrap(writer) {
            match writer.into_inner() {
                Ok(wav_writer) => wav_writer.finalize()?,
                Err(_e) => return Err(anyhow!("Failed to get WAV writer")),
            }
        }

        Ok(start_time.elapsed())
    }

    /// Build an audio stream for WAV recording
    fn build_stream<T>(
        &self,
        writer: Arc<Mutex<WavWriter<File>>>,
        silence_detector: Option<SilenceDetector>,
        should_stop: Arc<Mutex<bool>>,
    ) -> Result<cpal::Stream>
    where
        T: cpal::Sample + cpal::SizedSample + Send + 'static,
        f32: cpal::FromSample<T>,
    {
        let writer_clone = writer.clone();
        let silence_detector_clone = silence_detector.clone();

        let stream = self.device.build_input_stream(
            &self.config.clone().into(),
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                // Write samples to WAV file
                if let Ok(mut writer) = writer_clone.lock() {
                    for &sample in data {
                        let sample_f32: f32 = cpal::Sample::from_sample(sample);
                        writer
                            .write_sample((sample_f32 * i16::MAX as f32) as i16)
                            .ok();
                    }
                }

                // Check for silence
                if let Some(ref detector) = silence_detector_clone {
                    let has_sound = data.iter().any(|&sample| {
                        let sample_f32: f32 = cpal::Sample::from_sample(sample);
                        !detector.is_silent(sample_f32)
                    });

                    if has_sound {
                        detector.update_sound_time();
                    }
                }
            },
            move |err| {
                eprintln!("Audio device disconnected or stream error: {}", err);
                eprintln!("Recording stopped due to audio device error. Check device connection.");
                if let Ok(mut should_stop) = should_stop.lock() {
                    *should_stop = true;
                }
            },
            None,
        )?;

        Ok(stream)
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
            &self.config.clone().into(),
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
                        if let Some(ref mut analyzer) = spectrum_analyzer {
                            if let Some(bands) = analyzer.push_sample(sample) {
                                if let Some(ref tx) = spectrum_tx {
                                    let _ = tx.send(bands);
                                }
                            }
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
