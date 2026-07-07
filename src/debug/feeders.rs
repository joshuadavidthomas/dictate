use std::time::Duration;

use crate::spectrum::SPECTRUM_BANDS;

const SINE_SWEEP_PERIOD: Duration = Duration::from_secs(2);

pub(in crate::debug) const RECORDED_SPECTRUM_FRAMES: [[f32; SPECTRUM_BANDS]; 6] = [
    [0.04, 0.08, 0.18, 0.42, 0.74, 0.58, 0.32, 0.14],
    [0.06, 0.12, 0.28, 0.64, 0.88, 0.70, 0.36, 0.18],
    [0.03, 0.10, 0.34, 0.82, 0.96, 0.78, 0.44, 0.20],
    [0.02, 0.06, 0.24, 0.58, 0.86, 0.92, 0.62, 0.28],
    [0.01, 0.04, 0.16, 0.38, 0.66, 0.76, 0.52, 0.24],
    [0.00, 0.03, 0.10, 0.24, 0.48, 0.54, 0.34, 0.16],
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::debug) enum SpectrumSource {
    Silent,
    Constant(f32),
    SineSweep,
    Frames(&'static [[f32; SPECTRUM_BANDS]]),
}

impl SpectrumSource {
    pub(in crate::debug) fn frame_at(
        self,
        elapsed: Duration,
        frame_index: u64,
    ) -> [f32; SPECTRUM_BANDS] {
        match self {
            Self::Silent => [0.0; SPECTRUM_BANDS],
            Self::Constant(level) => [level.clamp(0.0, 1.0); SPECTRUM_BANDS],
            Self::SineSweep => sine_sweep_frame(elapsed),
            Self::Frames(frames) => recorded_frame(frames, frame_index),
        }
    }
}

fn sine_sweep_frame(elapsed: Duration) -> [f32; SPECTRUM_BANDS] {
    let cycles = elapsed.as_secs_f32() / SINE_SWEEP_PERIOD.as_secs_f32();

    std::array::from_fn(|band| {
        let phase = cycles + band as f32 / SPECTRUM_BANDS as f32;
        (0.5 + 0.5 * (phase * std::f32::consts::TAU).sin()).clamp(0.0, 1.0)
    })
}

fn recorded_frame(
    frames: &'static [[f32; SPECTRUM_BANDS]],
    frame_index: u64,
) -> [f32; SPECTRUM_BANDS] {
    if frames.is_empty() {
        return [0.0; SPECTRUM_BANDS];
    }

    let index = frame_index as usize % frames.len();
    frames[index].map(|band| band.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 0.000_001;

    #[test]
    fn sine_sweep_stays_in_range() {
        for frame in 0..120 {
            let elapsed = Duration::from_millis(frame * 16);
            let bands = SpectrumSource::SineSweep.frame_at(elapsed, frame);

            assert!(bands.iter().all(|band| (0.0..=1.0).contains(band)));
        }
    }

    #[test]
    fn sine_sweep_repeats_after_period() {
        let first = SpectrumSource::SineSweep.frame_at(Duration::ZERO, 0);
        let second = SpectrumSource::SineSweep.frame_at(SINE_SWEEP_PERIOD, 120);

        for (first, second) in first.into_iter().zip(second) {
            assert!((first - second).abs() <= EPSILON);
        }
    }

    #[test]
    fn constant_source_clamps_frame() {
        let bands = SpectrumSource::Constant(1.5).frame_at(Duration::ZERO, 0);

        assert_eq!(bands, [1.0; SPECTRUM_BANDS]);
    }
}
