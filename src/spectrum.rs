use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use rustfft::Fft;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use serde::Serialize;

const BASS_GATE_THRESHOLD: f32 = 0.30;
const SPEECH_GATE_THRESHOLD: f32 = 0.10;

pub const SPECTRUM_BANDS: usize = 8;
const FFT_SIZE: usize = 512;
const FFT_HOP_SIZE: usize = 128;

pub const DEFAULT_WAVEFORM_SMOOTHING: WaveformSmoothingConfig = WaveformSmoothingConfig {
    max_frame_time_secs: 0.05,
    rise_speed: 16.0,
    fall_speed: 10.0,
    visual_gate_on: 0.16,
    visual_gate_off: 0.08,
};

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

#[derive(Clone, Debug)]
pub struct SpectrumLevels {
    bands: Arc<[AtomicU32; SPECTRUM_BANDS]>,
}

impl Default for SpectrumLevels {
    fn default() -> Self {
        Self::new()
    }
}

impl SpectrumLevels {
    pub fn new() -> Self {
        Self {
            bands: Arc::new(std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits()))),
        }
    }

    pub fn set(&self, bands: [f32; SPECTRUM_BANDS]) {
        for (level, stored) in bands.into_iter().zip(self.bands.iter()) {
            stored.store(level.to_bits(), Ordering::Relaxed);
        }
    }

    pub fn bands(&self) -> [f32; SPECTRUM_BANDS] {
        std::array::from_fn(|index| f32::from_bits(self.bands[index].load(Ordering::Relaxed)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveformSmoothingConfig {
    pub max_frame_time_secs: f32,
    pub rise_speed: f32,
    pub fall_speed: f32,
    pub visual_gate_on: f32,
    pub visual_gate_off: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaveformGateState {
    Open,
    Closed,
}

impl WaveformGateState {
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveformAdvance {
    pub smoothed_bands: [f32; SPECTRUM_BANDS],
    pub gate_state: WaveformGateState,
}

pub fn advance_waveform_bands(
    displayed_bands: [f32; SPECTRUM_BANDS],
    visual_active: bool,
    target_bands: [f32; SPECTRUM_BANDS],
    frame_time_secs: f32,
    config: WaveformSmoothingConfig,
) -> WaveformAdvance {
    let frame_time = frame_time_secs.min(config.max_frame_time_secs);
    let peak = target_bands.iter().copied().fold(0.0, f32::max);
    let visual_active = if visual_active {
        peak >= config.visual_gate_off
    } else {
        peak >= config.visual_gate_on
    };
    let gated_bands = if visual_active {
        target_bands
    } else {
        [0.0; SPECTRUM_BANDS]
    };
    let smoothed_bands = std::array::from_fn(|index| {
        let displayed = displayed_bands[index];
        let target = gated_bands[index];
        let speed = if target > displayed {
            config.rise_speed
        } else {
            config.fall_speed
        };
        let blend = 1.0 - (-speed * frame_time).exp();

        displayed + (target - displayed) * blend
    });

    WaveformAdvance {
        smoothed_bands,
        gate_state: if visual_active {
            WaveformGateState::Open
        } else {
            WaveformGateState::Closed
        },
    }
}

pub struct SpectrumAnalyzer {
    sample_buffer: Vec<f32>,
    fft_input: Vec<Complex<f32>>,
    fft: Arc<dyn Fft<f32>>,
    window: [f32; FFT_SIZE],
    sample_rate: u32,
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
            fft_input: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            fft,
            window,
            sample_rate,
        }
    }

    pub fn push_sample(&mut self, sample: f32) -> Option<[f32; SPECTRUM_BANDS]> {
        self.sample_buffer.push(sample);

        if self.sample_buffer.len() >= FFT_SIZE {
            let bands = self.compute_spectrum();
            self.sample_buffer.copy_within(FFT_HOP_SIZE..FFT_SIZE, 0);
            self.sample_buffer.truncate(FFT_SIZE - FFT_HOP_SIZE);
            Some(bands)
        } else {
            None
        }
    }

    fn compute_spectrum(&mut self) -> [f32; SPECTRUM_BANDS] {
        for (index, (&sample, &window)) in self
            .sample_buffer
            .iter()
            .zip(self.window.iter())
            .enumerate()
        {
            self.fft_input[index] = Complex::new(sample * window, 0.0);
        }
        self.fft.process(&mut self.fft_input);

        let bin_width_hz = self.sample_rate as f32 / FFT_SIZE as f32;
        let analysis_bin_limit = FFT_SIZE / 2;
        let mut bands = [0.0; SPECTRUM_BANDS];

        for (index, band) in VISUAL_BANDS.iter().copied().enumerate() {
            bands[index] = band.level(&self.fft_input, bin_width_hz, analysis_bin_limit);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_gate_uses_on_and_off_thresholds() {
        let closed = advance_waveform_bands(
            [0.0; SPECTRUM_BANDS],
            false,
            [DEFAULT_WAVEFORM_SMOOTHING.visual_gate_on - 0.001; SPECTRUM_BANDS],
            0.016,
            DEFAULT_WAVEFORM_SMOOTHING,
        );
        assert_eq!(closed.gate_state, WaveformGateState::Closed);

        let opened = advance_waveform_bands(
            [0.0; SPECTRUM_BANDS],
            false,
            [DEFAULT_WAVEFORM_SMOOTHING.visual_gate_on; SPECTRUM_BANDS],
            0.016,
            DEFAULT_WAVEFORM_SMOOTHING,
        );
        assert_eq!(opened.gate_state, WaveformGateState::Open);

        let held_open = advance_waveform_bands(
            [0.0; SPECTRUM_BANDS],
            true,
            [DEFAULT_WAVEFORM_SMOOTHING.visual_gate_off; SPECTRUM_BANDS],
            0.016,
            DEFAULT_WAVEFORM_SMOOTHING,
        );
        assert_eq!(held_open.gate_state, WaveformGateState::Open);

        let closed_after_falling = advance_waveform_bands(
            [0.0; SPECTRUM_BANDS],
            true,
            [DEFAULT_WAVEFORM_SMOOTHING.visual_gate_off - 0.001; SPECTRUM_BANDS],
            0.016,
            DEFAULT_WAVEFORM_SMOOTHING,
        );
        assert_eq!(closed_after_falling.gate_state, WaveformGateState::Closed);
    }

    #[test]
    fn waveform_blend_converges_toward_target() {
        let mut displayed = [0.0; SPECTRUM_BANDS];
        let mut active = false;

        for _ in 0..80 {
            let advance = advance_waveform_bands(
                displayed,
                active,
                [1.0; SPECTRUM_BANDS],
                0.016,
                DEFAULT_WAVEFORM_SMOOTHING,
            );
            displayed = advance.smoothed_bands;
            active = advance.gate_state.is_open();
        }

        assert!(displayed.iter().all(|band| *band > 0.999));
    }
}
