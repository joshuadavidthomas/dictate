use anyhow::Result;
use anyhow::anyhow;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use hound::{WavSpec, WavWriter};
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Aggregates audio samples over a fixed time window to compute RMS level
/// Uses time-based windowing (32ms) rather than sample count for consistent update rates
pub struct LevelAggregator {
    window_samples: usize,
    sum_sq: f32,
    count: usize,
}

impl LevelAggregator {
    /// Create a new aggregator with a 32ms time window
    /// Window size adapts to sample rate to maintain consistent timing
    pub fn new(sample_rate: u32) -> Self {
        // Fixed 32ms time window (adaptive sample count)
        let window_samples = ((sample_rate as f32 * 0.032).round() as usize).max(128);
        Self {
            window_samples,
            sum_sq: 0.0,
            count: 0,
        }
    }
    
    /// Push an i16 sample and get RMS level when window is complete
    /// Returns Some(level) every ~32ms
    pub fn push_i16(&mut self, sample: i16) -> Option<f32> {
        // Normalize to [-1.0, 1.0]
        let normalized = (sample as f32) / (i16::MAX as f32);
        self.sum_sq += normalized * normalized;
        self.count += 1;
        
        if self.count >= self.window_samples {
            let rms = (self.sum_sq / self.count as f32).sqrt();
            
            // Reset for next window
            self.sum_sq = 0.0;
            self.count = 0;
            
            // Clamp to [0, 1] range
            // Could add optional log mapping here for better visual response:
            // let ui_level = (20.0 * rms.max(1e-5).log10() / -20.0).clamp(0.0, 1.0);
            Some(rms.clamp(0.0, 1.0))
        } else {
            None
        }
    }
}

pub struct AudioRecorder {
    device: Device,
    config: StreamConfig,
    sample_rate: u32,
}

#[derive(Debug)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub supported_sample_rates: Vec<u32>,
    pub supported_formats: Vec<SampleFormat>,
}

#[derive(Clone)]
pub struct SilenceDetector {
    threshold: f32,
    duration: Duration,
    last_sound_time: Arc<Mutex<Instant>>,
}

impl SilenceDetector {
    pub fn new(threshold: f32, duration: Duration) -> Self {
        Self {
            threshold,
            duration,
            last_sound_time: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub fn is_silent(&self, sample: f32) -> bool {
        sample.abs() < self.threshold
    }

    pub fn should_stop(&self) -> bool {
        let last_sound = match self.last_sound_time.lock() {
            Ok(guard) => *guard,
            Err(_) => {
                // Mutex poisoned, use current time as fallback
                Instant::now()
            }
        };
        last_sound.elapsed() > self.duration
    }

    pub fn update_sound_time(&self) {
        if let Ok(mut last_sound) = self.last_sound_time.lock() {
            *last_sound = Instant::now();
        }
    }
}

impl AudioRecorder {
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
    /// Optionally sends audio level updates via level_tx channel
    pub fn start_recording_background(
        &self,
        audio_buffer: Arc<Mutex<Vec<i16>>>,
        stop_signal: Arc<AtomicBool>,
        silence_detector: Option<SilenceDetector>,
        level_tx: Option<tokio::sync::mpsc::UnboundedSender<f32>>,
    ) -> Result<cpal::Stream> {
        let buffer_clone = audio_buffer.clone();
        let stop_clone = stop_signal.clone();
        let silence_detector_clone = silence_detector.clone();
        
        // Create level aggregator if we have a channel to send to
        let mut level_aggregator = level_tx.as_ref().map(|_| LevelAggregator::new(self.sample_rate));

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
                        
                        // Calculate and send level if aggregator exists
                        if let Some(ref mut agg) = level_aggregator {
                            if let Some(level) = agg.push_i16(sample_i16) {
                                if let Some(ref tx) = level_tx {
                                    let _ = tx.send(level);
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

    /// Convert audio buffer to WAV file
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
}
