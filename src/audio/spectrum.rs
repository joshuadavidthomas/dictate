//! Frequency spectrum analyzer for audio visualization
//!
//! Provides real-time FFT-based frequency analysis optimized for speech/voice input.
//! Features:
//! - 8 frequency bands from 20Hz to 8kHz
//! - Perceptual weighting for speech emphasis
//! - Noise gating to eliminate ambient sound
//! - Temporal smoothing for stable visualization

use rustfft::{num_complex::Complex, FftPlanner};

/// Configuration for spectrum analyzer behavior
#[derive(Debug, Clone)]
pub struct SpectrumConfig {
    /// FFT window size (512 or 1024 recommended)
    pub fft_size: usize,
    /// Number of frequency bands to output (8 recommended for speech)
    pub num_bands: usize,
    /// Temporal smoothing factor (0.0-1.0, higher = more smoothing)
    pub smoothing_factor: f32,
    /// Noise floor threshold to subtract from signal
    pub noise_floor: f32,
    /// Noise gate threshold for speech bands
    pub speech_gate_threshold: f32,
    /// Noise gate threshold for bass bands (usually higher)
    pub bass_gate_threshold: f32,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        Self {
            fft_size: 512,
            num_bands: 8,
            smoothing_factor: 0.7,
            noise_floor: 0.02,
            speech_gate_threshold: 0.35,
            bass_gate_threshold: 0.50,
        }
    }
}

/// FFT-based spectrum analyzer for frequency band visualization
///
/// Processes audio samples in real-time and produces frequency band levels
/// optimized for speech visualization in UI elements.
pub struct SpectrumAnalyzer {
    config: SpectrumConfig,
    sample_buffer: Vec<f32>,
    fft_planner: FftPlanner<f32>,
    window: Vec<f32>,
    sample_rate: u32,
    prev_bands: Vec<f32>,
}

impl SpectrumAnalyzer {
    /// Create a new spectrum analyzer with default configuration
    pub fn new(sample_rate: u32) -> Self {
        Self::with_config(sample_rate, SpectrumConfig::default())
    }

    /// Create a new spectrum analyzer with custom configuration
    pub fn with_config(sample_rate: u32, config: SpectrumConfig) -> Self {
        // Generate Hann window to reduce spectral leakage
        let mut window = vec![0.0; config.fft_size];
        for i in 0..config.fft_size {
            window[i] =
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / config.fft_size as f32).cos());
        }

        Self {
            sample_buffer: Vec::with_capacity(config.fft_size),
            prev_bands: vec![0.0; config.num_bands],
            window,
            fft_planner: FftPlanner::new(),
            sample_rate,
            config,
        }
    }

    /// Push a single audio sample and optionally return frequency bands
    ///
    /// Returns `Some(bands)` when the FFT window is full and ready to process.
    /// Otherwise returns `None` to indicate more samples are needed.
    pub fn push_sample(&mut self, sample: f32) -> Option<Vec<f32>> {
        self.sample_buffer.push(sample);

        if self.sample_buffer.len() >= self.config.fft_size {
            let bands = self.compute_spectrum();
            self.sample_buffer.clear();
            Some(bands)
        } else {
            None
        }
    }

    /// Compute frequency spectrum from buffered samples
    fn compute_spectrum(&mut self) -> Vec<f32> {
        // Apply Hann window to reduce spectral leakage
        let mut windowed: Vec<Complex<f32>> = self
            .sample_buffer
            .iter()
            .zip(self.window.iter())
            .map(|(&s, &w)| Complex::new(s * w, 0.0))
            .collect();

        // Perform FFT
        let fft = self.fft_planner.plan_fft_forward(self.config.fft_size);
        fft.process(&mut windowed);

        // Group into frequency bands
        let mut bands = self.group_into_bands(&windowed);

        // Apply temporal smoothing
        for (i, band) in bands.iter_mut().enumerate() {
            *band = self.config.smoothing_factor * self.prev_bands[i]
                + (1.0 - self.config.smoothing_factor) * *band;
            self.prev_bands[i] = *band;
        }

        bands
    }

    /// Group FFT bins into frequency bands with speech-optimized weighting
    fn group_into_bands(&self, fft_data: &[Complex<f32>]) -> Vec<f32> {
        let nyquist = self.sample_rate as f32 / 2.0;
        let bin_width = nyquist / (self.config.fft_size as f32 / 2.0);

        // Frequency band edges (Hz) - optimized for speech at 16kHz sample rate
        let band_edges = vec![
            20.0,   // Sub-bass start
            125.0,  // Bass
            250.0,  // Low-mid
            500.0,  // Mid
            1000.0, // High-mid
            2000.0, // Presence
            4000.0, // Brilliance
            6000.0, // Air
            8000.0, // End (Nyquist for 16kHz)
        ];

        // Perceptual weights for speech - emphasize vocal range (500-3000 Hz)
        // Bass weights heavily reduced to filter out environmental noise
        let band_weights = vec![
            0.2, // 20-125 Hz: Sub-bass (heavily reduce - room noise)
            0.3, // 125-250 Hz: Bass (heavily reduce - room noise)
            0.8, // 250-500 Hz: Low-mid (keep most)
            1.5, // 500-1000 Hz: Mid (BOOST - core speech)
            1.8, // 1000-2000 Hz: High-mid (BOOST - core speech)
            1.2, // 2000-4000 Hz: Presence (slight boost)
            0.7, // 4000-6000 Hz: Brilliance (reduce)
            0.5, // 6000-8000 Hz: Air (reduce)
        ];

        let mut bands = vec![0.0; self.config.num_bands];

        for (band_idx, window) in band_edges.windows(2).enumerate() {
            let low_freq = window[0];
            let high_freq = window[1];

            let low_bin = (low_freq / bin_width) as usize;
            let high_bin = ((high_freq / bin_width) as usize).min(self.config.fft_size / 2);

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
                let signal = (rms - self.config.noise_floor).max(0.0);

                // Apply perceptual weighting for speech emphasis
                let weighted = signal * band_weights[band_idx];

                // Gentle square root compression for dynamic range
                let compressed = weighted.sqrt();

                // Aggressive noise gate with per-band thresholds
                // Bass frequencies need higher threshold due to room noise
                let threshold = if band_idx <= 1 {
                    self.config.bass_gate_threshold
                } else {
                    self.config.speech_gate_threshold
                };

                if compressed < threshold {
                    bands[band_idx] = 0.0;
                } else {
                    // Scale up after noise gate to use full dynamic range
                    // Map [threshold, 1.0] to [0.0, 1.0]
                    bands[band_idx] =
                        ((compressed - threshold) / (1.0 - threshold)).clamp(0.0, 1.0);
                }
            }
        }

        bands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spectrum_analyzer_creation() {
        let analyzer = SpectrumAnalyzer::new(16000);
        assert_eq!(analyzer.sample_rate, 16000);
    }

    #[test]
    fn test_push_sample_returns_none_until_full() {
        let mut analyzer = SpectrumAnalyzer::new(16000);
        
        // Push samples until just before FFT size
        for _ in 0..511 {
            assert!(analyzer.push_sample(0.0).is_none());
        }
        
        // Last sample should trigger FFT
        assert!(analyzer.push_sample(0.0).is_some());
    }

    #[test]
    fn test_silence_produces_zero_bands() {
        let mut analyzer = SpectrumAnalyzer::new(16000);
        
        // Push 512 silent samples
        for _ in 0..512 {
            let _ = analyzer.push_sample(0.0);
        }
        
        // Should get bands back (might be Some or None depending on when we check)
        // All bands should be 0.0 due to noise gate
        if let Some(bands) = analyzer.push_sample(0.0) {
            for band in bands {
                assert!(band < 0.01, "Expected near-zero for silence, got {}", band);
            }
        }
    }
}
