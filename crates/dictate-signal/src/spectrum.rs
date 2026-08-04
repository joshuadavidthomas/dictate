use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use rustfft::Fft;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use serde::Serialize;

const BASS_RELATIVE_GATE: f32 = 0.30;
const SPEECH_RELATIVE_GATE: f32 = 0.10;
const FLOOR_FALL_BLEND: f32 = 0.25;
const FLOOR_RISE_BLEND: f32 = 0.02;
const STRUCTURED_FLOOR_RISE_BLEND: f32 = 0.0035;
const MIN_NOISE_FLOOR: f32 = 1.0e-12;
const INITIAL_FLOOR_FRACTION: f32 = 0.25;
const STRUCTURED_SIGNAL_PROMINENCE: f32 = 2.0;
const MIN_BAND_PEAK_FRACTION: f32 = 0.01;

pub const SPECTRUM_BANDS: usize = 8;
const FFT_SIZE: usize = 512;
const FFT_SIZE_F64: f64 = 512.0;
const FFT_HOP_SIZE: usize = 128;

pub const DEFAULT_WAVEFORM_SMOOTHING: WaveformSmoothingConfig = WaveformSmoothingConfig {
    max_frame_time_secs: 0.05,
    rise_speed: 16.0,
    fall_speed: 10.0,
    visual_gate_on: 0.65,
    visual_gate_off: 0.35,
};

const VISUAL_BANDS: [SpectrumBand; SPECTRUM_BANDS] = [
    SpectrumBand::new(20.0, 125.0, 0.2, BASS_RELATIVE_GATE),
    SpectrumBand::new(125.0, 250.0, 0.3, BASS_RELATIVE_GATE),
    SpectrumBand::new(250.0, 500.0, 1.2, SPEECH_RELATIVE_GATE),
    SpectrumBand::new(500.0, 1000.0, 2.5, SPEECH_RELATIVE_GATE),
    SpectrumBand::new(1000.0, 2000.0, 3.0, SPEECH_RELATIVE_GATE),
    SpectrumBand::new(2000.0, 4000.0, 2.5, SPEECH_RELATIVE_GATE),
    SpectrumBand::new(4000.0, 6000.0, 1.8, SPEECH_RELATIVE_GATE),
    SpectrumBand::new(6000.0, 8000.0, 1.5, SPEECH_RELATIVE_GATE),
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
    #[must_use]
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

    #[must_use]
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
    #[must_use]
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
    noise_floors: [Option<f32>; SPECTRUM_BANDS],
    sample_rate: u32,
}

impl SpectrumAnalyzer {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        let mut fft_planner = FftPlanner::new();
        let fft = fft_planner.plan_fft_forward(FFT_SIZE);
        let phase_step = 2.0 * std::f32::consts::PI / 512.0;
        let mut phase: f32 = 0.0;
        let window = std::array::from_fn(|_| {
            let value = 0.5 * (1.0 - phase.cos());
            phase += phase_step;
            value
        });

        Self {
            sample_buffer: Vec::with_capacity(FFT_SIZE),
            fft_input: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            fft,
            window,
            noise_floors: [None; SPECTRUM_BANDS],
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

        let bin_width_hz = f64::from(self.sample_rate) / FFT_SIZE_F64;
        let analysis_bin_limit = FFT_SIZE / 2;
        let rms_bands =
            VISUAL_BANDS.map(|band| band.rms(&self.fft_input, bin_width_hz, analysis_bin_limit));
        let mean_rms = rms_bands.iter().sum::<f32>() / 8.0;
        let peak_rms = rms_bands.iter().copied().fold(0.0, f32::max);
        let structured_signal =
            peak_rms >= mean_rms.max(MIN_NOISE_FLOOR) * STRUCTURED_SIGNAL_PROMINENCE;

        std::array::from_fn(|index| {
            let rms = rms_bands[index];
            let floor = self.noise_floors[index].get_or_insert(rms * INITIAL_FLOOR_FRACTION);
            if rms < *floor * 0.9 {
                *floor += (rms - *floor) * FLOOR_FALL_BLEND;
            } else {
                let rise_blend = if structured_signal {
                    STRUCTURED_FLOOR_RISE_BLEND
                } else {
                    FLOOR_RISE_BLEND
                };
                *floor += (rms - *floor) * rise_blend;
            }
            // Ignore FFT leakage whose ratio to a real tone is unstable near zero.
            if rms < peak_rms * MIN_BAND_PEAK_FRACTION {
                0.0
            } else {
                VISUAL_BANDS[index].relative_level(rms, *floor)
            }
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct SpectrumBand {
    low_hz: f32,
    high_hz: f32,
    display_boost: f32,
    relative_gate: f32,
}

impl SpectrumBand {
    const fn new(low_hz: f32, high_hz: f32, display_boost: f32, relative_gate: f32) -> Self {
        Self {
            low_hz,
            high_hz,
            display_boost,
            relative_gate,
        }
    }

    fn rms(self, fft_data: &[Complex<f32>], bin_width_hz: f64, analysis_bin_limit: usize) -> f32 {
        let mut bin_low_hz = 0.0;
        let mut sum_squares = 0.0;
        let mut bin_count = 0_u16;

        for bin in fft_data.iter().take(analysis_bin_limit) {
            let bin_high_hz = bin_low_hz + bin_width_hz;
            if bin_high_hz > f64::from(self.low_hz) && bin_low_hz < f64::from(self.high_hz) {
                sum_squares += bin.norm_sqr();
                bin_count += 1;
            }
            bin_low_hz = bin_high_hz;
        }

        if bin_count == 0 {
            return 0.0;
        }

        (sum_squares / f32::from(bin_count)).sqrt()
    }

    fn relative_level(self, rms: f32, noise_floor: f32) -> f32 {
        if rms <= noise_floor || rms <= MIN_NOISE_FLOOR {
            return 0.0;
        }

        let excess_ratio = rms / noise_floor.max(MIN_NOISE_FLOOR) - 1.0;
        let compressed = (excess_ratio * self.display_boost).sqrt();
        let relative = compressed / (1.0 + compressed);

        if relative < self.relative_gate {
            0.0
        } else {
            (relative - self.relative_gate) / (1.0 - self.relative_gate)
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

    #[test]
    fn quiet_modulated_speech_drives_the_meter_within_one_second() {
        let frames = analyze_waveform(&modulated_speech(0.003, 2));
        let frames_in_first_second = frames.iter().take_while(|frame| frame.time_secs <= 1.0);

        assert!(
            frames_in_first_second
                .into_iter()
                .any(|frame| frame.gate_state.is_open()
                    && frame.smoothed_bands.iter().any(|band| *band > 0.0))
        );
    }

    #[test]
    fn digital_silence_stays_flat() {
        let frames = analyze_waveform(&vec![0.0; 16_000 * 2]);

        assert!(frames.iter().all(|frame| {
            frame.gate_state == WaveformGateState::Closed
                && frame
                    .smoothed_bands
                    .iter()
                    .all(|band| band.abs() < f32::EPSILON)
        }));
    }

    #[test]
    fn low_level_constant_noise_stays_flat_after_floor_adapts() {
        let frames = analyze_waveform(&uniform_noise(0.0005, 2));

        assert!(
            frames
                .iter()
                .filter(|frame| frame.time_secs >= 1.0)
                .all(|frame| frame.gate_state == WaveformGateState::Closed)
        );
        assert!(
            frames
                .last()
                .expect("noise should produce analyzer frames")
                .smoothed_bands
                .iter()
                .all(|band| *band < 0.001)
        );
    }

    #[test]
    fn low_level_tonal_noise_eventually_adapts_to_flat() {
        let frames = analyze_waveform(&tone(1_500.0, 0.0005, 12));

        assert!(
            frames
                .iter()
                .filter(|frame| frame.time_secs >= 10.0)
                .all(|frame| frame.gate_state == WaveformGateState::Closed)
        );
    }

    #[test]
    fn constant_speech_stays_visible_after_floor_adaptation() {
        let frames = analyze_waveform(&constant_speech(0.003, 3));

        assert!(
            frames
                .iter()
                .filter(|frame| frame.time_secs >= 2.0)
                .all(|frame| frame.gate_state.is_open())
        );
    }

    #[test]
    fn loud_modulated_speech_drives_the_meter() {
        let frames = analyze_waveform(&modulated_speech(0.3, 1));

        assert!(frames.iter().any(|frame| frame.gate_state.is_open()));
    }

    #[derive(Clone, Copy)]
    struct MeterFrame {
        time_secs: f32,
        smoothed_bands: [f32; SPECTRUM_BANDS],
        gate_state: WaveformGateState,
    }

    #[allow(clippy::cast_precision_loss)]
    fn analyze_waveform(samples: &[f32]) -> Vec<MeterFrame> {
        let mut analyzer = SpectrumAnalyzer::new(16_000);
        let mut displayed = [0.0; SPECTRUM_BANDS];
        let mut active = false;
        let mut frames = Vec::new();
        let frame_time_secs = 128.0 / 16_000.0;

        for (sample_index, &sample) in samples.iter().enumerate() {
            let Some(target_bands) = analyzer.push_sample(sample) else {
                continue;
            };
            let advance = advance_waveform_bands(
                displayed,
                active,
                target_bands,
                frame_time_secs,
                DEFAULT_WAVEFORM_SMOOTHING,
            );
            displayed = advance.smoothed_bands;
            active = advance.gate_state.is_open();
            frames.push(MeterFrame {
                time_secs: sample_index as f32 / 16_000.0,
                smoothed_bands: displayed,
                gate_state: advance.gate_state,
            });
        }
        frames
    }

    #[allow(clippy::cast_precision_loss)]
    fn modulated_speech(amplitude: f32, seconds: usize) -> Vec<f32> {
        (0..16_000 * seconds)
            .map(|index| {
                let time = index as f32 / 16_000.0;
                let envelope = 0.55 + 0.45 * (2.0 * std::f32::consts::PI * 4.0 * time).sin();
                let tones = (2.0 * std::f32::consts::PI * 200.0 * time).sin()
                    + (2.0 * std::f32::consts::PI * 1_000.0 * time).sin()
                    + (2.0 * std::f32::consts::PI * 2_500.0 * time).sin();
                amplitude * envelope * tones / 3.0
            })
            .collect()
    }

    #[allow(clippy::cast_precision_loss)]
    fn tone(frequency: f32, amplitude: f32, seconds: usize) -> Vec<f32> {
        (0..16_000 * seconds)
            .map(|index| {
                let time = index as f32 / 16_000.0;
                amplitude * (2.0 * std::f32::consts::PI * frequency * time).sin()
            })
            .collect()
    }

    #[allow(clippy::cast_precision_loss)]
    fn constant_speech(amplitude: f32, seconds: usize) -> Vec<f32> {
        (0..16_000 * seconds)
            .map(|index| {
                let time = index as f32 / 16_000.0;
                let tones = (2.0 * std::f32::consts::PI * 200.0 * time).sin()
                    + (2.0 * std::f32::consts::PI * 1_000.0 * time).sin()
                    + (2.0 * std::f32::consts::PI * 2_500.0 * time).sin();
                amplitude * tones / 3.0
            })
            .collect()
    }

    #[allow(clippy::cast_precision_loss)]
    fn uniform_noise(amplitude: f32, seconds: usize) -> Vec<f32> {
        let mut state = 0x1234_5678_u32;
        (0..16_000 * seconds)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let unit = state as f32 / u32::MAX as f32;
                amplitude * (unit * 2.0 - 1.0)
            })
            .collect()
    }
}
