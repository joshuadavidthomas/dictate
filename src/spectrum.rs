use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

pub const SPECTRUM_BANDS: usize = 8;
const FFT_SIZE: usize = 512;

const SPEECH_BANDS: [BandVisualization; SPECTRUM_BANDS] = [
    BandVisualization::new(20.0, 125.0, 0.2, BandType::Bass),
    BandVisualization::new(125.0, 250.0, 0.3, BandType::Bass),
    BandVisualization::new(250.0, 500.0, 1.2, BandType::Speech),
    BandVisualization::new(500.0, 1000.0, 2.5, BandType::Speech),
    BandVisualization::new(1000.0, 2000.0, 3.0, BandType::Speech),
    BandVisualization::new(2000.0, 4000.0, 2.5, BandType::Speech),
    BandVisualization::new(4000.0, 6000.0, 1.8, BandType::Speech),
    BandVisualization::new(6000.0, 8000.0, 1.5, BandType::Speech),
];

pub struct SpectrumAnalyzer {
    sample_buffer: Vec<f32>,
    fft_planner: FftPlanner<f32>,
    window: Vec<f32>,
    sample_rate: u32,
    noise_gate: NoiseGate,
    previous_frame_output: [f32; SPECTRUM_BANDS],
}

impl SpectrumAnalyzer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_buffer: Vec::with_capacity(FFT_SIZE),
            previous_frame_output: [0.0; SPECTRUM_BANDS],
            window: generate_hann_window(FFT_SIZE),
            fft_planner: FftPlanner::new(),
            sample_rate,
            noise_gate: NoiseGate::default(),
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
        let mut windowed = apply_window(&self.sample_buffer, &self.window);
        let fft = self.fft_planner.plan_fft_forward(FFT_SIZE);
        fft.process(&mut windowed);

        let fft_params = FftParams::new(self.sample_rate, FFT_SIZE);
        let processing = SignalProcessing::new(self.noise_gate);
        let mut bands = [0.0; SPECTRUM_BANDS];

        for (index, band) in SPEECH_BANDS.iter().copied().enumerate() {
            let level = band.process(&windowed, &fft_params, &processing);
            let smoothed = apply_temporal_smoothing(level, self.previous_frame_output[index]);
            bands[index] = smoothed;
            self.previous_frame_output[index] = smoothed;
        }

        bands
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BandType {
    Bass,
    Speech,
}

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

    fn nyquist(&self) -> f32 {
        self.sample_rate as f32 / 2.0
    }

    fn bin_width(&self) -> f32 {
        self.nyquist() / (self.fft_size as f32 / 2.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BinRange {
    low: usize,
    high: usize,
}

impl BinRange {
    const fn new(low: usize, high: usize) -> Self {
        Self { low, high }
    }

    fn calculate_rms(&self, fft_data: &[Complex<f32>]) -> f32 {
        if self.low >= self.high {
            return 0.0;
        }

        let sum_squares: f32 = fft_data[self.low..self.high]
            .iter()
            .map(|complex| {
                let magnitude = complex.norm();
                magnitude * magnitude
            })
            .sum();

        let count = (self.high - self.low) as f32;
        (sum_squares / count).sqrt()
    }
}

#[derive(Debug, Clone, Copy)]
struct NoiseGate {
    noise_floor: f32,
    bass_threshold: f32,
    speech_threshold: f32,
}

impl NoiseGate {
    const fn new(noise_floor: f32, bass_threshold: f32, speech_threshold: f32) -> Self {
        Self {
            noise_floor,
            bass_threshold,
            speech_threshold,
        }
    }

    fn threshold_for(&self, band_type: BandType) -> f32 {
        match band_type {
            BandType::Bass => self.bass_threshold,
            BandType::Speech => self.speech_threshold,
        }
    }

    fn gate(&self, signal: f32, band_type: BandType) -> f32 {
        let threshold = self.threshold_for(band_type);
        if signal < threshold {
            0.0
        } else {
            ((signal - threshold) / (1.0 - threshold)).clamp(0.0, 1.0)
        }
    }
}

impl Default for NoiseGate {
    fn default() -> Self {
        Self::new(0.005, 0.30, 0.10)
    }
}

#[derive(Debug, Clone, Copy)]
struct FrequencyBand {
    low_hz: f32,
    high_hz: f32,
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

    fn to_bin_range(self, params: &FftParams) -> BinRange {
        let bin_width = params.bin_width();
        let low_bin = (self.low_hz / bin_width) as usize;
        let high_bin = ((self.high_hz / bin_width) as usize).min(params.fft_size / 2);

        BinRange::new(low_bin, high_bin)
    }
}

#[derive(Debug, Clone, Copy)]
struct BandVisualization {
    band: FrequencyBand,
    display_boost: f32,
}

impl BandVisualization {
    const fn new(low_hz: f32, high_hz: f32, display_boost: f32, band_type: BandType) -> Self {
        Self {
            band: FrequencyBand::new(low_hz, high_hz, band_type),
            display_boost,
        }
    }

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

#[derive(Debug, Clone, Copy)]
struct SignalProcessing {
    noise_gate: NoiseGate,
}

impl SignalProcessing {
    const fn new(noise_gate: NoiseGate) -> Self {
        Self { noise_gate }
    }

    fn process(&self, rms: f32, weight: f32, band_type: BandType) -> f32 {
        let signal = (rms - self.noise_gate.noise_floor).max(0.0);
        let weighted = signal * weight;
        let compressed = weighted.sqrt();
        self.noise_gate.gate(compressed, band_type)
    }
}

fn generate_hann_window(size: usize) -> Vec<f32> {
    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / size as f32).cos()))
        .collect()
}

fn apply_window(samples: &[f32], window: &[f32]) -> Vec<Complex<f32>> {
    samples
        .iter()
        .zip(window.iter())
        .map(|(&sample, &window)| Complex::new(sample * window, 0.0))
        .collect()
}

fn apply_temporal_smoothing(current: f32, previous: f32) -> f32 {
    const SMOOTHING_FACTOR: f32 = 0.7;
    SMOOTHING_FACTOR * previous + (1.0 - SMOOTHING_FACTOR) * current
}
