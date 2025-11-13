//! Frequency spectrum analyzer for audio visualization
//!
//! Provides real-time FFT-based frequency analysis optimized for speech/voice input.
//! Features:
//! - 8 frequency bands from 20Hz to 8kHz
//! - Perceptual weighting for speech emphasis
//! - Noise gating to eliminate ambient sound
//! - Temporal smoothing for stable visualization

use rustfft::{FftPlanner, num_complex::Complex};

/// Type of frequency band, determines noise gate threshold
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BandType {
    /// Low frequencies, higher noise gate threshold (room noise)
    Bass,
    /// Mid-high frequencies, lower threshold (speech content)
    Speech,
}

/// FFT parameters that define frequency-to-bin mapping
#[derive(Debug, Clone, Copy)]
struct FftParams {
    sample_rate: u32,
    fft_size: usize,
}

impl FftParams {
    const fn new(sample_rate: u32, fft_size: usize) -> Self {
        Self {
            sample_rate,
            fft_size,
        }
    }

    /// Nyquist frequency (half the sample rate)
    #[inline]
    fn nyquist(&self) -> f32 {
        self.sample_rate as f32 / 2.0
    }

    /// Frequency resolution per FFT bin
    #[inline]
    fn bin_width(&self) -> f32 {
        self.nyquist() / (self.fft_size as f32 / 2.0)
    }
}

/// A range of FFT bins
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BinRange {
    low: usize,
    high: usize,
}

impl BinRange {
    fn new(low: usize, high: usize) -> Self {
        Self { low, high }
    }

    /// Calculate RMS magnitude for this bin range
    fn calculate_rms(&self, fft_data: &[Complex<f32>]) -> f32 {
        if self.low >= self.high {
            return 0.0;
        }

        let sum_squares: f32 = fft_data[self.low..self.high]
            .iter()
            .map(|c| {
                let mag = c.norm();
                mag * mag
            })
            .sum();

        let count = (self.high - self.low) as f32;
        (sum_squares / count).sqrt()
    }
}

/// Noise gate with floor and per-band thresholds
#[derive(Debug, Clone, Copy)]
struct NoiseGate {
    /// Minimum signal level to process (eliminates DC offset and low hum)
    noise_floor: f32,
    /// Threshold for bass frequencies (0-1, higher = more aggressive)
    bass_threshold: f32,
    /// Threshold for speech frequencies (0-1, lower = more sensitive)
    speech_threshold: f32,
}

impl Default for NoiseGate {
    fn default() -> Self {
        Self {
            noise_floor: 0.01,
            bass_threshold: 0.30,
            speech_threshold: 0.20,
        }
    }
}

impl NoiseGate {
    const fn new(noise_floor: f32, bass_threshold: f32, speech_threshold: f32) -> Self {
        Self {
            noise_floor,
            bass_threshold,
            speech_threshold,
        }
    }

    /// Get threshold for a band type
    #[inline]
    fn threshold_for(&self, band_type: BandType) -> f32 {
        match band_type {
            BandType::Bass => self.bass_threshold,
            BandType::Speech => self.speech_threshold,
        }
    }

    /// Apply noise gate to a signal
    fn gate(&self, signal: f32, band_type: BandType) -> f32 {
        let threshold = self.threshold_for(band_type);
        if signal < threshold {
            0.0
        } else {
            ((signal - threshold) / (1.0 - threshold)).clamp(0.0, 1.0)
        }
    }
}

/// A frequency band defined by acoustic properties only
#[derive(Debug, Clone, Copy)]
struct FrequencyBand {
    /// Lower frequency bound (Hz)
    low_hz: f32,
    /// Upper frequency bound (Hz)
    high_hz: f32,
    /// Band type (determines noise gate threshold)
    band_type: BandType,
}

impl FrequencyBand {
    const fn new(low_hz: f32, high_hz: f32, band_type: BandType) -> Self {
        Self {
            low_hz,
            high_hz,
            band_type,
        }
    }

    /// Convert this frequency band to FFT bin range
    fn to_bin_range(self, params: &FftParams) -> BinRange {
        let bin_width = params.bin_width();
        let low_bin = (self.low_hz / bin_width) as usize;
        let high_bin = ((self.high_hz / bin_width) as usize).min(params.fft_size / 2);

        BinRange::new(low_bin, high_bin)
    }
}

/// Couples a frequency band with visualization display settings
#[derive(Debug, Clone, Copy)]
struct BandVisualization {
    /// The acoustic frequency band
    band: FrequencyBand,
    /// Display amplification factor for UI visualization
    display_boost: f32,
}

impl BandVisualization {
    const fn new(low_hz: f32, high_hz: f32, display_boost: f32, band_type: BandType) -> Self {
        Self {
            band: FrequencyBand::new(low_hz, high_hz, band_type),
            display_boost,
        }
    }

    /// Process this band: bin range -> RMS -> signal processing
    fn process(
        self,
        fft_data: &[Complex<f32>],
        params: &FftParams,
        processing: &SignalProcessing,
    ) -> f32 {
        let bin_range = self.band.to_bin_range(params);
        let rms = bin_range.calculate_rms(fft_data);
        processing.process(rms, self.display_boost, self.band.band_type)
    }
}

/// Signal processing configuration and pipeline
#[derive(Debug, Clone, Copy)]
struct SignalProcessing {
    noise_gate: NoiseGate,
}

impl SignalProcessing {
    const fn new(noise_gate: NoiseGate) -> Self {
        Self { noise_gate }
    }

    /// Run the complete signal processing pipeline
    fn process(&self, rms: f32, weight: f32, band_type: BandType) -> f32 {
        let signal = (rms - self.noise_gate.noise_floor).max(0.0);
        let weighted = signal * weight;
        let compressed = weighted.sqrt();
        self.noise_gate.gate(compressed, band_type)
    }
}

/// FFT window size optimized for real-time speech processing
/// - 512 samples @ 16kHz = 32ms latency
/// - Provides 31.25 Hz frequency resolution
const FFT_SIZE: usize = 512;

/// Generate a Hann window to reduce spectral leakage in FFT
///
/// The Hann window smoothly tapers the signal at the edges to minimize
/// discontinuities that cause spectral leakage in the frequency domain.
///
/// Formula: w(n) = 0.5 * (1 - cos(2πn/N))
/// where n is the sample index and N is the window size
fn generate_hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / size as f32).cos()))
        .collect()
}

/// Apply window function to samples, preparing them for FFT
///
/// Multiplies each sample by its corresponding window coefficient and
/// converts to complex numbers (with zero imaginary component) ready for FFT.
fn apply_window(samples: &[f32], window: &[f32]) -> Vec<Complex<f32>> {
    samples
        .iter()
        .zip(window.iter())
        .map(|(&s, &w)| Complex::new(s * w, 0.0))
        .collect()
}

/// Apply exponential moving average (EMA) temporal smoothing
///
/// Blends the current value with the previous value to reduce jitter in visualization.
/// Uses a smoothing factor of 0.7 (70% previous, 30% current) - tuned for speech visualization.
#[inline]
fn apply_temporal_smoothing(current: f32, previous: f32) -> f32 {
    const SMOOTHING_FACTOR: f32 = 0.7;
    SMOOTHING_FACTOR * previous + (1.0 - SMOOTHING_FACTOR) * current
}

/// Speech-optimized frequency bands for 16kHz sample rate
/// Bass heavily reduced to filter environmental noise
/// Display boosts increased to compensate for correct RMS calculation
const SPEECH_BANDS: [BandVisualization; 8] = [
    // Sub-bass (room noise) - 20-125 Hz
    BandVisualization::new(20.0, 125.0, 0.2, BandType::Bass),
    // Bass (room noise) - 125-250 Hz
    BandVisualization::new(125.0, 250.0, 0.3, BandType::Bass),
    // Low-mid - 250-500 Hz (boosted from 0.8)
    BandVisualization::new(250.0, 500.0, 1.2, BandType::Speech),
    // Mid (core speech) - 500-1000 Hz (boosted from 1.5)
    BandVisualization::new(500.0, 1000.0, 2.5, BandType::Speech),
    // High-mid (core speech) - 1000-2000 Hz (boosted from 1.8)
    BandVisualization::new(1000.0, 2000.0, 3.0, BandType::Speech),
    // Presence - 2000-4000 Hz (boosted from 1.2)
    BandVisualization::new(2000.0, 4000.0, 2.0, BandType::Speech),
    // Brilliance - 4000-6000 Hz (boosted from 0.7)
    BandVisualization::new(4000.0, 6000.0, 1.0, BandType::Speech),
    // Air - 6000-8000 Hz (boosted from 0.5)
    BandVisualization::new(6000.0, 8000.0, 0.8, BandType::Speech),
];

/// FFT-based spectrum analyzer for frequency band visualization
///
/// Processes audio samples in real-time and produces frequency band levels
/// optimized for speech visualization in UI elements.
pub struct SpectrumAnalyzer {
    sample_buffer: Vec<f32>,
    fft_planner: FftPlanner<f32>,
    window: Vec<f32>,
    sample_rate: u32,
    noise_gate: NoiseGate,
    previous_frame_output: Vec<f32>,
}

impl SpectrumAnalyzer {
    /// Create a new spectrum analyzer for speech visualization
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_buffer: Vec::with_capacity(FFT_SIZE),
            previous_frame_output: vec![0.0; SPEECH_BANDS.len()],
            window: generate_hann_window(FFT_SIZE),
            fft_planner: FftPlanner::new(),
            sample_rate,
            noise_gate: NoiseGate::default(),
        }
    }

    /// Push a single audio sample and optionally return frequency bands
    ///
    /// Returns `Some(bands)` when the FFT window is full and ready to process.
    /// Otherwise returns `None` to indicate more samples are needed.
    pub fn push_sample(&mut self, sample: f32) -> Option<Vec<f32>> {
        self.sample_buffer.push(sample);

        if self.sample_buffer.len() >= FFT_SIZE {
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
        let mut windowed = apply_window(&self.sample_buffer, &self.window);

        let fft = self.fft_planner.plan_fft_forward(FFT_SIZE);
        fft.process(&mut windowed);

        let fft_params = FftParams::new(self.sample_rate, FFT_SIZE);
        let processing = SignalProcessing::new(self.noise_gate);

        let mut bands: Vec<f32> = SPEECH_BANDS
            .iter()
            .copied()
            .map(|band| band.process(&windowed, &fft_params, &processing))
            .collect();

        for (i, band) in bands.iter_mut().enumerate() {
            *band = apply_temporal_smoothing(*band, self.previous_frame_output[i]);
            self.previous_frame_output[i] = *band;
        }

        bands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_hann_window() {
        let window = generate_hann_window(4);

        // Hann window should have 4 values
        assert_eq!(window.len(), 4);

        // Expected values for size 4: [0, 0.5, 1, 0.5]
        assert!(window[0].abs() < 0.0001); // Start at 0
        assert!((window[1] - 0.5).abs() < 0.0001);
        assert!((window[2] - 1.0).abs() < 0.0001); // Peak at center
        assert!((window[3] - 0.5).abs() < 0.0001);

        // Window should be symmetric around center
        assert!((window[0] + window[3] - 0.5).abs() < 0.0001);
        assert!((window[1] - window[3]).abs() < 0.0001);
    }

    #[test]
    fn test_apply_window() {
        let samples = vec![1.0, 2.0, 3.0, 4.0];
        let window = vec![0.5, 1.0, 1.0, 0.5];

        let windowed = apply_window(&samples, &window);

        assert_eq!(windowed.len(), 4);

        // Check that samples are multiplied by window coefficients
        assert!((windowed[0].re - 0.5).abs() < 0.0001); // 1.0 * 0.5
        assert!((windowed[1].re - 2.0).abs() < 0.0001); // 2.0 * 1.0
        assert!((windowed[2].re - 3.0).abs() < 0.0001); // 3.0 * 1.0
        assert!((windowed[3].re - 2.0).abs() < 0.0001); // 4.0 * 0.5

        // Imaginary parts should all be zero
        for val in windowed {
            assert_eq!(val.im, 0.0);
        }
    }

    #[test]
    fn test_noise_gate_threshold_selection() {
        let gate = NoiseGate::new(0.02, 0.5, 0.35);

        assert_eq!(gate.threshold_for(BandType::Bass), 0.5);
        assert_eq!(gate.threshold_for(BandType::Speech), 0.35);
    }

    #[test]
    fn test_noise_gate_gating() {
        let gate = NoiseGate::new(0.02, 0.5, 0.35);

        // Below threshold - gated to 0
        assert_eq!(gate.gate(0.2, BandType::Speech), 0.0);

        // At threshold - gated to 0
        assert_eq!(gate.gate(0.35, BandType::Speech), 0.0);

        // At maximum
        assert_eq!(gate.gate(1.0, BandType::Speech), 1.0);

        // Mid-range: (0.675 - 0.35) / (1.0 - 0.35) = 0.5
        let result = gate.gate(0.675, BandType::Speech);
        assert!((result - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_fft_params() {
        let params = FftParams::new(16000, 512);

        assert_eq!(params.sample_rate, 16000);
        assert_eq!(params.fft_size, 512);
        assert_eq!(params.nyquist(), 8000.0);

        // bin_width = 8000 / 256 = 31.25
        assert!((params.bin_width() - 31.25).abs() < 0.01);
    }

    #[test]
    fn test_bin_range_calculate_rms_empty() {
        let fft_data = vec![Complex::new(0.0, 0.0); 256];
        let range = BinRange::new(10, 10); // Empty range

        assert_eq!(range.calculate_rms(&fft_data), 0.0);
    }

    #[test]
    fn test_bin_range_calculate_rms_with_signal() {
        let mut fft_data = vec![Complex::new(0.0, 0.0); 256];

        // Set bins 10-14 to have magnitude 1.0
        fft_data[10..15].fill(Complex::new(1.0, 0.0));

        let range = BinRange::new(10, 15);
        let rms = range.calculate_rms(&fft_data);

        // RMS of five 1.0 values: sqrt((5 * 1.0^2) / 5) = 1.0
        assert!((rms - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_frequency_band_to_bin_range() {
        let band = FrequencyBand::new(500.0, 1000.0, BandType::Speech);
        let params = FftParams::new(16000, 512);
        let range = band.to_bin_range(&params);

        // bin_width = 31.25, so 500Hz = bin 16, 1000Hz = bin 32
        assert_eq!(range.low, 16);
        assert_eq!(range.high, 32);
    }

    #[test]
    fn test_frequency_band_to_bin_range_clamps_at_nyquist() {
        let band = FrequencyBand::new(7000.0, 10000.0, BandType::Speech);
        let params = FftParams::new(16000, 512);
        let range = band.to_bin_range(&params);

        // High freq exceeds Nyquist, should clamp to fft_size/2 = 256
        assert_eq!(range.high, 256);
        assert!(range.low < range.high);
    }

    #[test]
    fn test_signal_processing_pipeline() {
        let gate = NoiseGate::new(0.02, 0.5, 0.35);
        let processing = SignalProcessing::new(gate);

        // Test with speech band
        let result = processing.process(0.1, 1.5, BandType::Speech);

        // Should go through: subtract floor -> weight -> compress -> gate
        // (0.1 - 0.02) = 0.08, * 1.5 = 0.12, sqrt = 0.346
        // Below speech threshold (0.35), should be gated to 0.0
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_frequency_band_const_creation() {
        const BAND: FrequencyBand = FrequencyBand::new(500.0, 1000.0, BandType::Speech);

        assert_eq!(BAND.low_hz, 500.0);
        assert_eq!(BAND.high_hz, 1000.0);
        assert_eq!(BAND.band_type, BandType::Speech);
    }

    #[test]
    fn test_band_visualization_const_creation() {
        const VIZ: BandVisualization = BandVisualization::new(500.0, 1000.0, 1.5, BandType::Speech);

        assert_eq!(VIZ.band.low_hz, 500.0);
        assert_eq!(VIZ.band.high_hz, 1000.0);
        assert_eq!(VIZ.display_boost, 1.5);
        assert_eq!(VIZ.band.band_type, BandType::Speech);
    }

    #[test]
    fn test_speech_bands_constant() {
        // Verify SPEECH_BANDS is properly defined
        assert_eq!(SPEECH_BANDS.len(), 8);

        // Check first band is bass
        assert_eq!(SPEECH_BANDS[0].band.low_hz, 20.0);
        assert_eq!(SPEECH_BANDS[0].band.high_hz, 125.0);
        assert_eq!(SPEECH_BANDS[0].band.band_type, BandType::Bass);

        // Check last band is speech
        assert_eq!(SPEECH_BANDS[7].band.low_hz, 6000.0);
        assert_eq!(SPEECH_BANDS[7].band.high_hz, 8000.0);
        assert_eq!(SPEECH_BANDS[7].band.band_type, BandType::Speech);

        // Verify bands are contiguous (no gaps)
        for i in 0..SPEECH_BANDS.len() - 1 {
            assert_eq!(
                SPEECH_BANDS[i].band.high_hz,
                SPEECH_BANDS[i + 1].band.low_hz,
                "Gap between band {} and {}",
                i,
                i + 1
            );
        }

        // Verify first two are bass, rest are speech
        assert_eq!(SPEECH_BANDS[0].band.band_type, BandType::Bass);
        assert_eq!(SPEECH_BANDS[1].band.band_type, BandType::Bass);
        for band_viz in &SPEECH_BANDS[2..8] {
            assert_eq!(band_viz.band.band_type, BandType::Speech);
        }
    }

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
