//! Audio capture and processing
//!
//! Everything audio-related in one place: device management, recording,
//! silence detection, spectrum analysis, and WAV output.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, StreamConfig};
use hound::{SampleFormat, WavSpec, WavWriter};
use rustfft::{FftPlanner, num_complex::Complex};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// ============================================================================
// Constants
// ============================================================================

/// Valid sample rates (Hz). 16kHz is optimal for speech recognition.
pub const VALID_SAMPLE_RATES: [u32; 5] = [8000, 16000, 22050, 44100, 48000];

/// Number of spectrum bands for visualization
pub const SPECTRUM_BANDS: usize = 16;

/// FFT size for spectrum analysis
const FFT_SIZE: usize = 512;

/// How often to emit spectrum data (in samples at the current sample rate)
const SPECTRUM_INTERVAL_SAMPLES: usize = 1024;

// ============================================================================
// Public Types
// ============================================================================

/// Information about an available audio input device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDevice {
    pub name: String,
    pub supported_sample_rates: Vec<u32>,
}

/// Sample rate option with UI metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleRateOption {
    pub value: u32,
    pub is_recommended: bool,
}

/// A recording session that owns the stream and buffers
///
/// This abstraction emerged naturally: these four things always travel together
/// and have a clear lifecycle (start -> accumulate samples -> stop -> get audio).
pub struct RecordingSession {
    stream: cpal::Stream,
    buffer: Arc<Mutex<Vec<i16>>>,
    stop_signal: Arc<AtomicBool>,
    start_time: std::time::Instant,
}

// ============================================================================
// Sample Rate Validation
// ============================================================================

pub fn validate_sample_rate(rate: u32) -> Result<u32> {
    if VALID_SAMPLE_RATES.contains(&rate) {
        Ok(rate)
    } else {
        Err(anyhow!(
            "Unsupported sample rate: {}. Valid rates: {:?}",
            rate,
            VALID_SAMPLE_RATES
        ))
    }
}

pub fn sample_rate_options() -> Vec<SampleRateOption> {
    VALID_SAMPLE_RATES
        .iter()
        .map(|&rate| SampleRateOption {
            value: rate,
            is_recommended: rate == 16000,
        })
        .collect()
}

// ============================================================================
// Device Management
// ============================================================================

/// List all available audio input devices
pub fn list_devices() -> Result<Vec<AudioDevice>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();

    for device in host.input_devices()? {
        let name = device.name().unwrap_or_else(|_| "Unknown".into());

        // Skip the virtual "default" device alias
        if name == "default" {
            continue;
        }

        let supported_sample_rates = VALID_SAMPLE_RATES
            .iter()
            .filter(|&&rate| device_supports_rate(&device, rate))
            .copied()
            .collect();

        devices.push(AudioDevice {
            name,
            supported_sample_rates,
        });
    }

    Ok(devices)
}

fn device_supports_rate(device: &Device, rate: u32) -> bool {
    device
        .supported_input_configs()
        .map(|mut configs| {
            configs.any(|config| {
                let min = config.min_sample_rate().0;
                let max = config.max_sample_rate().0;
                rate >= min && rate <= max
            })
        })
        .unwrap_or(false)
}

fn get_device(name: Option<&str>) -> Result<Device> {
    let host = cpal::default_host();

    match name {
        Some(name) => host
            .input_devices()?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or_else(|| anyhow!("Audio device '{}' not found", name)),
        None => host
            .default_input_device()
            .ok_or_else(|| anyhow!("No default input device found")),
    }
}

fn get_config(device: &Device, target_rate: u32) -> Result<StreamConfig> {
    let supported = device.supported_input_configs()?;

    // Find config closest to target rate
    let mut best: Option<(StreamConfig, u32)> = None;

    for range in supported {
        let min = range.min_sample_rate().0;
        let max = range.max_sample_rate().0;

        let rate = target_rate.clamp(min, max);
        let diff = rate.abs_diff(target_rate);

        if best.is_none() || diff < best.as_ref().unwrap().1 {
            let config = range.with_sample_rate(cpal::SampleRate(rate));
            best = Some((config.into(), diff));
        }
    }

    best.map(|(c, _)| c)
        .ok_or_else(|| anyhow!("No suitable audio configuration found"))
}

// ============================================================================
// Recording Session
// ============================================================================

impl RecordingSession {
    /// Start a new recording session
    ///
    /// Optionally provides spectrum data via the spectrum_tx channel for visualization.
    pub fn start(
        device_name: Option<&str>,
        sample_rate: u32,
        spectrum_tx: Option<tokio::sync::mpsc::UnboundedSender<[f32; SPECTRUM_BANDS]>>,
    ) -> Result<Self> {
        let device = get_device(device_name)?;
        let config = get_config(&device, sample_rate)?;
        let actual_rate = config.sample_rate.0;

        let buffer = Arc::new(Mutex::new(Vec::new()));
        let stop_signal = Arc::new(AtomicBool::new(false));

        let buffer_clone = buffer.clone();
        let stop_clone = stop_signal.clone();

        // Spectrum analyzer state (only if channel provided)
        let mut spectrum = spectrum_tx.as_ref().map(|_| SpectrumAnalyzer::new(actual_rate));

        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if stop_clone.load(Ordering::Acquire) {
                    return;
                }

                if let Ok(mut buf) = buffer_clone.lock() {
                    for &sample in data {
                        // Convert f32 to i16 for storage
                        let sample_i16 = (sample * i16::MAX as f32) as i16;
                        buf.push(sample_i16);

                        // Update spectrum if enabled
                        if let Some(ref mut analyzer) = spectrum {
                            if let Some(bands) = analyzer.push_sample(sample) {
                                if let Some(ref tx) = spectrum_tx {
                                    let _ = tx.send(bands);
                                }
                            }
                        }
                    }
                }
            },
            |err| eprintln!("Recording error: {}", err),
            None,
        )?;

        stream.play()?;

        Ok(Self {
            stream,
            buffer,
            stop_signal,
            start_time: std::time::Instant::now(),
        })
    }

    /// Stop recording and return the captured audio samples
    pub fn stop(self) -> Vec<i16> {
        self.stop_signal.store(true, Ordering::Release);
        drop(self.stream);

        // Small delay to ensure callback finishes
        std::thread::sleep(std::time::Duration::from_millis(50));

        self.buffer.lock().unwrap().clone()
    }

    /// Get elapsed recording time
    pub fn elapsed(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }

    /// Get elapsed time in milliseconds
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }
}

// ============================================================================
// Audio Level (for device testing)
// ============================================================================

/// Record briefly and return average audio level (0.0 to 1.0)
pub fn get_audio_level(device_name: Option<&str>, sample_rate: u32) -> Result<f32> {
    let session = RecordingSession::start(device_name, sample_rate, None)?;

    std::thread::sleep(std::time::Duration::from_millis(100));

    let samples = session.stop();

    if samples.is_empty() {
        return Ok(0.0);
    }

    // RMS calculation
    let sum_squares: f64 = samples
        .iter()
        .map(|&s| {
            let normalized = s as f64 / i16::MAX as f64;
            normalized * normalized
        })
        .sum();

    Ok((sum_squares / samples.len() as f64).sqrt() as f32)
}

// ============================================================================
// WAV Output
// ============================================================================

/// Write audio samples to a WAV file
pub fn write_wav(samples: &[i16], path: &Path, sample_rate: u32) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut writer = WavWriter::create(path, spec)?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;

    Ok(())
}

// ============================================================================
// Spectrum Analysis
// ============================================================================

struct SpectrumAnalyzer {
    fft_buffer: Vec<Complex<f32>>,
    sample_count: usize,
    planner: FftPlanner<f32>,
    sample_rate: u32,
}

impl SpectrumAnalyzer {
    fn new(sample_rate: u32) -> Self {
        Self {
            fft_buffer: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            sample_count: 0,
            planner: FftPlanner::new(),
            sample_rate,
        }
    }

    /// Push a sample, returns spectrum bands when ready
    fn push_sample(&mut self, sample: f32) -> Option<[f32; SPECTRUM_BANDS]> {
        let idx = self.sample_count % FFT_SIZE;
        self.fft_buffer[idx] = Complex::new(sample, 0.0);
        self.sample_count += 1;

        // Emit spectrum at regular intervals
        if self.sample_count % SPECTRUM_INTERVAL_SAMPLES == 0 {
            Some(self.compute_bands())
        } else {
            None
        }
    }

    fn compute_bands(&mut self) -> [f32; SPECTRUM_BANDS] {
        let fft = self.planner.plan_fft_forward(FFT_SIZE);

        let mut buffer = self.fft_buffer.clone();
        fft.process(&mut buffer);

        // Convert to magnitude spectrum (only positive frequencies)
        let magnitudes: Vec<f32> = buffer[..FFT_SIZE / 2]
            .iter()
            .map(|c| (c.re * c.re + c.im * c.im).sqrt())
            .collect();

        // Map to bands with logarithmic frequency scaling
        let mut bands = [0.0f32; SPECTRUM_BANDS];
        let nyquist = self.sample_rate as f32 / 2.0;
        let bin_hz = nyquist / (FFT_SIZE / 2) as f32;

        for (i, band) in bands.iter_mut().enumerate() {
            // Logarithmic frequency bands
            let f_low = 80.0 * (8000.0 / 80.0_f32).powf(i as f32 / SPECTRUM_BANDS as f32);
            let f_high = 80.0 * (8000.0 / 80.0_f32).powf((i + 1) as f32 / SPECTRUM_BANDS as f32);

            let bin_low = (f_low / bin_hz) as usize;
            let bin_high = ((f_high / bin_hz) as usize).min(magnitudes.len() - 1);

            if bin_high > bin_low {
                let sum: f32 = magnitudes[bin_low..=bin_high].iter().sum();
                let avg = sum / (bin_high - bin_low + 1) as f32;
                // Normalize to 0-1 range with some headroom
                *band = (avg * 4.0).min(1.0);
            }
        }

        bands
    }
}

// ============================================================================
// Silence Detection
// ============================================================================

/// Silence detector for automatic recording stop
pub struct SilenceDetector {
    threshold: f32,
    silence_duration_ms: u64,
    last_sound_time: AtomicU64,
}

impl SilenceDetector {
    pub fn new(threshold: f32, silence_duration_ms: u64) -> Self {
        Self {
            threshold,
            silence_duration_ms,
            last_sound_time: AtomicU64::new(0),
        }
    }

    /// Check if a sample is below the silence threshold
    pub fn is_silent(&self, sample: f32) -> bool {
        sample.abs() < self.threshold
    }

    /// Update the last time sound was detected
    pub fn update_sound_time(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        self.last_sound_time.store(now, Ordering::Release);
    }

    /// Check if silence has exceeded the threshold duration
    pub fn should_stop(&self) -> bool {
        let last = self.last_sound_time.load(Ordering::Acquire);
        if last == 0 {
            return false;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        now.saturating_sub(last) > self.silence_duration_ms
    }
}
