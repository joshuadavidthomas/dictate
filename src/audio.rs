use anyhow::Result;
use anyhow::anyhow;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use hound::{WavSpec, WavWriter};
use rustfft::{FftPlanner, num_complex::Complex};
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

/// FFT-based spectrum analyzer for frequency band visualization
pub struct SpectrumAnalyzer {
    fft_size: usize,
    sample_buffer: Vec<f32>,
    fft_planner: FftPlanner<f32>,
    num_bands: usize,
    window: Vec<f32>,
    sample_rate: u32,
    smoothing_factor: f32,
    prev_bands: Vec<f32>,
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32, fft_size: usize, num_bands: usize) -> Self {
        let mut window = vec![0.0; fft_size];
        // Generate Hann window to reduce spectral leakage
        for i in 0..fft_size {
            window[i] = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos());
        }
        
        Self {
            fft_size,
            sample_buffer: Vec::with_capacity(fft_size),
            fft_planner: FftPlanner::new(),
            num_bands,
            window,
            sample_rate,
            smoothing_factor: 0.7,
            prev_bands: vec![0.0; num_bands],
        }
    }
    
    /// Push a sample and return frequency bands if buffer is full
    pub fn push_sample(&mut self, sample: f32) -> Option<Vec<f32>> {
        self.sample_buffer.push(sample);
        
        if self.sample_buffer.len() >= self.fft_size {
            let bands = self.compute_spectrum();
            self.sample_buffer.clear();
            Some(bands)
        } else {
            None
        }
    }
    
    fn compute_spectrum(&mut self) -> Vec<f32> {
        // Apply Hann window to reduce spectral leakage
        let mut windowed: Vec<Complex<f32>> = self.sample_buffer
            .iter()
            .zip(self.window.iter())
            .map(|(&s, &w)| Complex::new(s * w, 0.0))
            .collect();
        
        // Perform FFT
        let fft = self.fft_planner.plan_fft_forward(self.fft_size);
        fft.process(&mut windowed);
        
        // Group into frequency bands
        let mut bands = self.group_into_bands(&windowed);
        
        // Apply temporal smoothing
        for (i, band) in bands.iter_mut().enumerate() {
            *band = self.smoothing_factor * self.prev_bands[i] + (1.0 - self.smoothing_factor) * *band;
            self.prev_bands[i] = *band;
        }
        
        bands
    }
    
    fn group_into_bands(&self, fft_data: &[Complex<f32>]) -> Vec<f32> {
        let nyquist = self.sample_rate as f32 / 2.0;
        let bin_width = nyquist / (self.fft_size as f32 / 2.0);
        
        // Define frequency band edges (Hz)
        // For 16kHz sample rate, Nyquist = 8kHz
        let band_edges = vec![
            20.0,    // Sub-bass start
            125.0,   // Bass
            250.0,   // Low-mid
            500.0,   // Mid
            1000.0,  // High-mid
            2000.0,  // Presence
            4000.0,  // Brilliance
            6000.0,  // Air
            8000.0,  // End (Nyquist for 16kHz)
        ];
        
        // Perceptual weights for speech - emphasize vocal range (500-3000 Hz)
        // These weights reduce bass dominance and boost speech-relevant frequencies
        // Bass weights heavily reduced to filter out fan/AC rumble
        let band_weights = vec![
            0.2,  // 20-125 Hz: Sub-bass (heavily reduce - room noise)
            0.3,  // 125-250 Hz: Bass (heavily reduce - room noise)
            0.8,  // 250-500 Hz: Low-mid (keep most)
            1.5,  // 500-1000 Hz: Mid (BOOST - core speech)
            1.8,  // 1000-2000 Hz: High-mid (BOOST - core speech)
            1.2,  // 2000-4000 Hz: Presence (slight boost)
            0.7,  // 4000-6000 Hz: Brilliance (reduce)
            0.5,  // 6000-8000 Hz: Air (reduce)
        ];
        
        let mut bands = vec![0.0; self.num_bands];
        
        for (band_idx, window) in band_edges.windows(2).enumerate() {
            let low_freq = window[0];
            let high_freq = window[1];
            
            let low_bin = (low_freq / bin_width) as usize;
            let high_bin = ((high_freq / bin_width) as usize).min(self.fft_size / 2);
            
            let mut sum = 0.0;
            let mut count = 0;
            
            for bin in low_bin..high_bin {
                let magnitude = fft_data[bin].norm();
                sum += magnitude;
                count += 1;
            }
            
            if count > 0 {
                let rms = (sum / count as f32).sqrt();
                
                // Subtract noise floor to eliminate ambient noise (fans, AC, etc.)
                const NOISE_FLOOR: f32 = 0.02;
                let signal = (rms - NOISE_FLOOR).max(0.0);
                
                // Apply perceptual weighting for speech emphasis
                let weighted = signal * band_weights[band_idx];
                
                // Gentle square root compression for dynamic range
                let compressed = weighted.sqrt();
                
                // Aggressive noise gate with per-band thresholds
                // Bass frequencies need higher threshold due to room noise (fans, AC, rumble)
                let threshold = if band_idx <= 1 {
                    0.50  // Bass bands (20-250 Hz): much stricter gate
                } else {
                    0.35  // Speech bands: normal gate
                };
                
                if compressed < threshold {
                    bands[band_idx] = 0.0;
                } else {
                    // Scale up after noise gate to use full dynamic range
                    // Map [threshold, 1.0] to [0.0, 1.0]
                    bands[band_idx] = ((compressed - threshold) / (1.0 - threshold)).clamp(0.0, 1.0);
                }
            }
        }
        
        bands
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
    /// Optionally sends spectrum updates via spectrum_tx channel
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
        let mut spectrum_analyzer = spectrum_tx.as_ref().map(|_| SpectrumAnalyzer::new(self.sample_rate, 512, 8));

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
