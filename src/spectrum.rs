use std::sync::Arc;

use rustfft::Fft;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

const SMOOTHING_FACTOR: f32 = 0.7;
const BASS_GATE_THRESHOLD: f32 = 0.30;
const SPEECH_GATE_THRESHOLD: f32 = 0.10;

pub const SPECTRUM_BANDS: usize = 8;
const FFT_SIZE: usize = 512;

const VISUAL_BANDS: [SpectrumBand; SPECTRUM_BANDS] = [
    SpectrumBand::new(20.0, 125.0, 0.2, BASS_GATE_THRESHOLD),
    SpectrumBand::new(125.0, 250.0, 0.3, BASS_GATE_THRESHOLD),
    SpectrumBand::new(250.0, 500.0, 1.2, SPEECH_GATE_THRESHOLD),
    SpectrumBand::new(500.0, 1000.0, 2.5, SPEECH_GATE_THRESHOLD),
    SpectrumBand::new(1000.0, 2000.0, 3.0, SPEECH_GATE_THRESHOLD),
    SpectrumBand::new(2000.0, 4000.0, 2.5, SPEECH_GATE_THRESHOLD),
    SpectrumBand::new(4000.0, 6000.0, 1.8, SPEECH_GATE_THRESHOLD),
    SpectrumBand::new(6000.0, 8000.0, 1.5, SPEECH_GATE_THRESHOLD),
];

pub struct SpectrumAnalyzer {
    sample_buffer: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    window: [f32; FFT_SIZE],
    sample_rate: u32,
    previous_frame_output: [f32; SPECTRUM_BANDS],
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        let mut fft_planner = FftPlanner::new();
        let fft = fft_planner.plan_fft_forward(FFT_SIZE);
        let window = std::array::from_fn(|index| {
            let phase = 2.0 * std::f32::consts::PI * index as f32 / FFT_SIZE as f32;
            0.5 * (1.0 - phase.cos())
        });

        Self {
            sample_buffer: Vec::with_capacity(FFT_SIZE),
            previous_frame_output: [0.0; SPECTRUM_BANDS],
            fft,
            window,
            sample_rate,
        }
    }

    pub fn push_sample(&mut self, sample: f32) -> Option<[f32; SPECTRUM_BANDS]> {
        self.sample_buffer.push(sample);

        if self.sample_buffer.len() >= FFT_SIZE {
            let bands = self.compute_spectrum();
            self.sample_buffer.clear();
            Some(bands)
        } else {
            None
        }
    }

    fn compute_spectrum(&mut self) -> [f32; SPECTRUM_BANDS] {
        let mut fft_input = self
            .sample_buffer
            .iter()
            .zip(self.window.iter())
            .map(|(&sample, &window)| Complex::new(sample * window, 0.0))
            .collect::<Vec<_>>();
        self.fft.process(&mut fft_input);

        let bin_width_hz = self.sample_rate as f32 / FFT_SIZE as f32;
        let analysis_bin_limit = FFT_SIZE / 2;
        let mut bands = [0.0; SPECTRUM_BANDS];

        for (index, band) in VISUAL_BANDS.iter().copied().enumerate() {
            let level = band.level(&fft_input, bin_width_hz, analysis_bin_limit);
            let smoothed = SMOOTHING_FACTOR * self.previous_frame_output[index]
                + (1.0 - SMOOTHING_FACTOR) * level;
            bands[index] = smoothed;
            self.previous_frame_output[index] = smoothed;
        }

        bands
    }
}

#[derive(Debug, Clone, Copy)]
struct SpectrumBand {
    low_hz: f32,
    high_hz: f32,
    display_boost: f32,
    gate_threshold: f32,
}

impl SpectrumBand {
    const fn new(low_hz: f32, high_hz: f32, display_boost: f32, gate_threshold: f32) -> Self {
        Self {
            low_hz,
            high_hz,
            display_boost,
            gate_threshold,
        }
    }

    fn level(self, fft_data: &[Complex<f32>], bin_width_hz: f32, analysis_bin_limit: usize) -> f32 {
        let start = ((self.low_hz / bin_width_hz).ceil().max(0.0) as usize).min(analysis_bin_limit);
        let end = ((self.high_hz / bin_width_hz).ceil().max(0.0) as usize).min(analysis_bin_limit);
        if start >= end {
            return 0.0;
        }

        let bins = &fft_data[start..end];
        let sum_squares: f32 = bins.iter().map(Complex::norm_sqr).sum();
        let rms = (sum_squares / bins.len() as f32).sqrt();
        let noise_floor = 0.005;
        let signal = (rms - noise_floor).max(0.0);
        let compressed = (signal * self.display_boost).sqrt();

        if compressed < self.gate_threshold {
            0.0
        } else {
            ((compressed - self.gate_threshold) / (1.0 - self.gate_threshold)).clamp(0.0, 1.0)
        }
    }
}
