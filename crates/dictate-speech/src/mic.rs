use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use cpal::BufferSize;
use cpal::Device;
use cpal::FromSample;
use cpal::I24;
use cpal::Sample;
use cpal::SampleFormat;
use cpal::SizedSample;
use cpal::StreamConfig;
use cpal::SupportedBufferSize;
use cpal::SupportedStreamConfig;
use cpal::SupportedStreamConfigRange;
use cpal::U24;
use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::traits::StreamTrait;
use dictate_signal::SPECTRUM_BANDS;
use dictate_signal::SpectrumAnalyzer;
use rtrb::Consumer;
use rtrb::Producer;
use rtrb::RingBuffer;

const AUDIO_RING_SAMPLES: usize = 192_000;
const WORKER_BATCH_SAMPLES: usize = 256;
const EMPTY_RING_SLEEP: Duration = Duration::from_millis(1);
const TARGET_CALLBACK_DURATION: Duration = Duration::from_millis(16);

#[derive(Debug)]
pub struct MicrophoneStreamError(cpal::Error);

impl std::fmt::Display for MicrophoneStreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for MicrophoneStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub struct Mic {
    stream: Option<cpal::Stream>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Eq, PartialEq)]
enum DeviceSelection {
    Default,
    Requested(usize),
    FallbackDefault { requested: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpectrumUpdate {
    Emit,
    Skip,
}

pub trait CaptureHandler: Send + Sync + 'static {
    fn samples(&self, samples: &[f32]) -> SpectrumUpdate;
    fn spectrum(&self, bands: [f32; SPECTRUM_BANDS]);
    fn stream_error(&self, error: &MicrophoneStreamError);
}

impl Drop for Mic {
    fn drop(&mut self) {
        drop(self.stream.take());
        if let Some(worker) = self.worker.take() {
            drop(worker.join());
        }
    }
}

pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .map(|device| input_device_name(&device))
        .transpose()?;
    let mut seen = HashSet::new();
    let mut devices = Vec::new();
    if let Some(name) = default_name.as_ref() {
        seen.insert(name.clone());
        devices.push(InputDeviceInfo {
            name: name.clone(),
            is_default: true,
        });
    }
    for device in host.input_devices()? {
        let name = input_device_name(&device)?;
        if seen.insert(name.clone()) {
            devices.push(InputDeviceInfo {
                name,
                is_default: false,
            });
        }
    }
    Ok(devices)
}

pub fn capture<H>(
    output_sample_rate: u32,
    requested_device: Option<&str>,
    handler: H,
) -> Result<Mic>
where
    H: CaptureHandler,
{
    if output_sample_rate == 0 {
        return Err(anyhow!("microphone output sample rate must be non-zero"));
    }

    let host = cpal::default_host();
    let default_device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device found"))?;
    let default_name = input_device_name(&default_device)?;
    let device = if requested_device.is_none() {
        default_device
    } else {
        let mut seen = HashSet::new();
        let mut available_devices = Vec::new();
        for device in host.input_devices()? {
            let name = input_device_name(&device)?;
            if seen.insert(name.clone()) {
                available_devices.push((name, device));
            }
        }
        let available_names = available_devices
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        let selection = if requested_device == Some(default_name.as_str()) {
            DeviceSelection::Default
        } else {
            select_input_device(requested_device, &available_names)
        };
        match selection {
            DeviceSelection::Default => default_device,
            DeviceSelection::Requested(index) => available_devices
                .into_iter()
                .nth(index)
                .map(|(_, device)| device)
                .ok_or_else(|| anyhow!("selected input device index {index} was unavailable"))?,
            DeviceSelection::FallbackDefault { requested } => {
                eprintln!(
                    "requested input device {requested:?} not found; using default {default_name:?}"
                );
                default_device
            }
        }
    };
    let config = input_config(&device, output_sample_rate)?;
    let stream_config = stream_config_with_target_buffer(&config);
    let requested_fixed_buffer = matches!(stream_config.buffer_size, BufferSize::Fixed(_));
    let handler = Arc::new(handler);

    match capture_with_config(
        &device,
        &config,
        &stream_config,
        output_sample_rate,
        Arc::clone(&handler),
    ) {
        Ok(mic) => Ok(mic),
        Err(error) if requested_fixed_buffer => {
            eprintln!("fixed input buffer size rejected: {error:#}; falling back to default");
            let fallback_config = config.config();
            capture_with_config(
                &device,
                &config,
                &fallback_config,
                output_sample_rate,
                handler,
            )
        }
        Err(error) => Err(error),
    }
}

fn input_device_name(device: &Device) -> Result<String> {
    Ok(device.description()?.name().to_owned())
}

fn select_input_device(requested: Option<&str>, available: &[String]) -> DeviceSelection {
    let Some(requested) = requested else {
        return DeviceSelection::Default;
    };
    available
        .iter()
        .position(|name| name == requested)
        .map_or_else(
            || DeviceSelection::FallbackDefault {
                requested: requested.to_owned(),
            },
            DeviceSelection::Requested,
        )
}

fn capture_with_config<H>(
    device: &Device,
    supported_config: &SupportedStreamConfig,
    stream_config: &StreamConfig,
    output_sample_rate: u32,
    handler: Arc<H>,
) -> Result<Mic>
where
    H: CaptureHandler,
{
    let input_sample_rate = stream_config.sample_rate;
    let resampler = AudioResampler::new(input_sample_rate, output_sample_rate)?;
    let (producer, consumer) = RingBuffer::<f32>::new(AUDIO_RING_SAMPLES);
    let dropped_samples = Arc::new(AtomicU64::new(0));

    let device_name = input_device_name(device)?;
    eprintln!(
        "capturing microphone audio from {device_name} at {}Hz, {} channel(s), {}, {:?} buffer",
        stream_config.sample_rate,
        stream_config.channels,
        supported_config.sample_format(),
        stream_config.buffer_size
    );

    let stream_error_handler = Arc::clone(&handler);
    let stream = build_input_stream_for_format(
        device,
        supported_config,
        stream_config,
        producer,
        Arc::clone(&dropped_samples),
        move |error| {
            stream_error_handler.stream_error(&MicrophoneStreamError(error));
        },
    )?;

    stream.play()?;
    let worker = thread::spawn(move || {
        audio_worker(
            consumer,
            input_sample_rate,
            output_sample_rate,
            resampler,
            &dropped_samples,
            handler,
        );
    });

    Ok(Mic {
        stream: Some(stream),
        worker: Some(worker),
    })
}

fn stream_config_with_target_buffer(config: &SupportedStreamConfig) -> StreamConfig {
    let mut stream_config = config.config();
    let duration_nanos = TARGET_CALLBACK_DURATION.as_nanos();
    let frames_numerator = u128::from(config.sample_rate()) * duration_nanos;
    let rounded_frames = (frames_numerator + 500_000_000) / 1_000_000_000;
    let target_frames = u32::try_from(rounded_frames.max(1)).map_or(u32::MAX, |frames| frames);
    stream_config.buffer_size = match *config.buffer_size() {
        SupportedBufferSize::Range { min, max } => BufferSize::Fixed(target_frames.clamp(min, max)),
        SupportedBufferSize::Unknown => BufferSize::Fixed(target_frames),
    };
    stream_config
}

fn input_config(device: &Device, output_sample_rate: u32) -> Result<SupportedStreamConfig> {
    if let Some(config) =
        preferred_input_config(device.supported_input_configs()?, output_sample_rate)
    {
        return Ok(config);
    }

    let config = device.default_input_config()?;
    if config.sample_format().is_dsd() {
        return Err(anyhow!(
            "default input device uses unsupported DSD sample format {}",
            config.sample_format()
        ));
    }

    Ok(config)
}

fn preferred_input_config(
    configs: impl IntoIterator<Item = SupportedStreamConfigRange>,
    output_sample_rate: u32,
) -> Option<SupportedStreamConfig> {
    configs
        .into_iter()
        .filter(|config| !config.sample_format().is_dsd())
        .filter(|config| config.contains_rate(output_sample_rate))
        .max_by(SupportedStreamConfigRange::cmp_default_heuristics)
        .map(|config| config.with_sample_rate(output_sample_rate))
}

fn build_input_stream_for_format<E>(
    device: &Device,
    supported_config: &SupportedStreamConfig,
    stream_config: &StreamConfig,
    producer: Producer<f32>,
    dropped_samples: Arc<AtomicU64>,
    stream_error: E,
) -> Result<cpal::Stream>
where
    E: FnMut(cpal::Error) + Send + 'static,
{
    match supported_config.sample_format() {
        SampleFormat::I8 => build_input_stream::<i8, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::I16 => build_input_stream::<i16, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::I24 => build_input_stream::<I24, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::I32 => build_input_stream::<i32, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::I64 => build_input_stream::<i64, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::U8 => build_input_stream::<u8, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::U16 => build_input_stream::<u16, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::U24 => build_input_stream::<U24, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::U32 => build_input_stream::<u32, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::U64 => build_input_stream::<u64, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::F32 => build_input_stream::<f32, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        SampleFormat::F64 => build_input_stream::<f64, E>(
            device,
            stream_config,
            producer,
            dropped_samples,
            stream_error,
        ),
        format @ (SampleFormat::DsdU8 | SampleFormat::DsdU16 | SampleFormat::DsdU32) | format => {
            Err(anyhow!("unsupported input sample format {format}"))
        }
    }
}

fn build_input_stream<T, E>(
    device: &Device,
    stream_config: &StreamConfig,
    mut producer: Producer<f32>,
    dropped_samples: Arc<AtomicU64>,
    stream_error: E,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
    E: FnMut(cpal::Error) + Send + 'static,
{
    let channels = usize::from(stream_config.channels);

    Ok(device.build_input_stream(
        *stream_config,
        move |data: &[T], _| {
            for frame in data.chunks(channels) {
                let (sum, count) = frame.iter().fold((0.0, 0.0), |(sum, count), sample| {
                    (sum + f32::from_sample(*sample), count + 1.0)
                });
                let sample = sum / count;
                if producer.push(sample).is_err() {
                    dropped_samples.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        stream_error,
        None,
    )?)
}

fn audio_worker<H>(
    consumer: Consumer<f32>,
    input_sample_rate: u32,
    output_sample_rate: u32,
    resampler: AudioResampler,
    dropped_samples: &AtomicU64,
    handler: Arc<H>,
) where
    H: CaptureHandler,
{
    run_audio_worker(
        consumer,
        input_sample_rate,
        output_sample_rate,
        resampler,
        dropped_samples,
        move |samples, spectrum_analyzer| {
            if handler.samples(samples) == SpectrumUpdate::Skip {
                return;
            }
            for &sample in samples {
                if let Some(bands) = spectrum_analyzer.push_sample(sample) {
                    handler.spectrum(bands);
                }
            }
        },
    );
}

fn run_audio_worker(
    mut consumer: Consumer<f32>,
    input_sample_rate: u32,
    output_sample_rate: u32,
    mut resampler: AudioResampler,
    dropped_samples: &AtomicU64,
    mut sink: impl FnMut(&[f32], &mut SpectrumAnalyzer),
) {
    let mut overflow_warned = false;
    let mut input = Vec::with_capacity(WORKER_BATCH_SAMPLES);
    let mut samples = Vec::with_capacity(WORKER_BATCH_SAMPLES);
    let mut spectrum_analyzer = SpectrumAnalyzer::new(output_sample_rate);

    loop {
        input.clear();
        while input.len() < WORKER_BATCH_SAMPLES {
            let Ok(sample) = consumer.pop() else {
                break;
            };
            input.push(sample);
        }

        if input.is_empty() {
            if consumer.is_abandoned() {
                break;
            }
            warn_on_first_overflow(dropped_samples, input_sample_rate, &mut overflow_warned);
            thread::sleep(EMPTY_RING_SLEEP);
            continue;
        }

        resampler.process_into(&input, &mut samples);
        sink(&samples, &mut spectrum_analyzer);

        warn_on_first_overflow(dropped_samples, input_sample_rate, &mut overflow_warned);
    }

    resampler.flush_into(&mut samples);
    if !samples.is_empty() {
        sink(&samples, &mut spectrum_analyzer);
    }

    let total_dropped_samples = dropped_samples.load(Ordering::Relaxed);
    if total_dropped_samples > 0 {
        eprintln!(
            "mic ring buffer overflowed; dropped {total_dropped_samples} samples (~{}ms of audio)",
            dropped_duration_ms(total_dropped_samples, input_sample_rate)
        );
    }
}

fn warn_on_first_overflow(
    dropped_samples: &AtomicU64,
    input_sample_rate: u32,
    overflow_warned: &mut bool,
) {
    if *overflow_warned {
        return;
    }
    let total_dropped_samples = dropped_samples.load(Ordering::Relaxed);
    if total_dropped_samples > 0 {
        eprintln!(
            "mic ring buffer overflowed; dropped {total_dropped_samples} samples (~{}ms of audio)",
            dropped_duration_ms(total_dropped_samples, input_sample_rate)
        );
        *overflow_warned = true;
    }
}

fn dropped_duration_ms(samples: u64, rate: u32) -> u64 {
    samples * 1000 / u64::from(rate)
}

enum AudioResampler {
    PassThrough,
    Bandlimited(sherpa_onnx::LinearResampler),
}

impl AudioResampler {
    fn new(input_sample_rate: u32, output_sample_rate: u32) -> Result<Self> {
        if input_sample_rate == output_sample_rate {
            return Ok(Self::PassThrough);
        }

        let input_rate = i32::try_from(input_sample_rate)
            .map_err(|error| anyhow!("microphone input sample rate exceeds i32: {error}"))?;
        let output_rate = i32::try_from(output_sample_rate)
            .map_err(|error| anyhow!("microphone output sample rate exceeds i32: {error}"))?;
        let resampler = sherpa_onnx::LinearResampler::create(input_rate, output_rate)
            .ok_or_else(|| anyhow!("could not create microphone resampler"))?;
        Ok(Self::Bandlimited(resampler))
    }

    fn process_into(&mut self, input: &[f32], output: &mut Vec<f32>) {
        output.clear();
        match self {
            Self::PassThrough => output.extend_from_slice(input),
            Self::Bandlimited(resampler) => output.extend(resampler.resample(input, false)),
        }
    }

    fn flush_into(&mut self, output: &mut Vec<f32>) {
        output.clear();
        if let Self::Bandlimited(resampler) = self {
            output.extend(resampler.resample(&[], true));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const TEST_SAMPLE_RATE: u32 = 16_000;

    #[derive(Default)]
    struct TestCaptureHandler {
        samples: Mutex<Vec<f32>>,
    }

    impl CaptureHandler for TestCaptureHandler {
        fn samples(&self, samples: &[f32]) -> SpectrumUpdate {
            self.samples
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(samples);
            SpectrumUpdate::Emit
        }

        fn spectrum(&self, _bands: [f32; SPECTRUM_BANDS]) {}

        fn stream_error(&self, error: &MicrophoneStreamError) {
            panic!("unexpected stream error: {error}");
        }
    }

    fn supported_input_config(
        sample_format: SampleFormat,
        min_sample_rate: u32,
        max_sample_rate: u32,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(
            1,
            min_sample_rate,
            max_sample_rate,
            SupportedBufferSize::Unknown,
            sample_format,
        )
    }

    #[test]
    fn preferred_input_config_chooses_precision_independent_of_iteration_order() {
        for configs in [
            [
                supported_input_config(SampleFormat::U8, 16_000, 48_000),
                supported_input_config(SampleFormat::I16, 16_000, 48_000),
            ],
            [
                supported_input_config(SampleFormat::I16, 16_000, 48_000),
                supported_input_config(SampleFormat::U8, 16_000, 48_000),
            ],
        ] {
            let selected = preferred_input_config(configs, TEST_SAMPLE_RATE)
                .expect("a matching input config should be selected");

            assert_eq!(selected.sample_format(), SampleFormat::I16);
            assert_eq!(selected.sample_rate(), TEST_SAMPLE_RATE);
        }
    }

    #[test]
    fn preferred_input_config_ignores_better_formats_outside_the_target_rate() {
        let selected = preferred_input_config(
            [
                supported_input_config(SampleFormat::F32, 48_000, 48_000),
                supported_input_config(SampleFormat::I16, 16_000, 16_000),
            ],
            TEST_SAMPLE_RATE,
        )
        .expect("the exact-rate input config should be selected");

        assert_eq!(selected.sample_format(), SampleFormat::I16);
        assert_eq!(selected.sample_rate(), TEST_SAMPLE_RATE);
    }

    #[test]
    fn audio_worker_exits_when_producer_is_dropped() {
        let (producer, consumer) = RingBuffer::<f32>::new(4);
        drop(producer);

        let dropped_samples = Arc::new(AtomicU64::new(0));
        audio_worker(
            consumer,
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            AudioResampler::new(TEST_SAMPLE_RATE, TEST_SAMPLE_RATE).expect("valid rates"),
            &dropped_samples,
            Arc::new(TestCaptureHandler::default()),
        );
    }

    #[test]
    fn audio_worker_forwards_resampled_batches_through_handler_seam() {
        let (mut producer, consumer) = RingBuffer::<f32>::new(4);
        producer.push(0.25).expect("ring should accept sample");
        producer.push(0.5).expect("ring should accept sample");
        drop(producer);
        let handler = Arc::new(TestCaptureHandler::default());

        audio_worker(
            consumer,
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            AudioResampler::new(TEST_SAMPLE_RATE, TEST_SAMPLE_RATE).expect("valid rates"),
            &AtomicU64::new(0),
            Arc::clone(&handler),
        );

        assert_eq!(
            *handler
                .samples
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [0.25, 0.5]
        );
    }

    #[test]
    fn input_device_selection_uses_an_exact_name_match() {
        let available = vec!["Dock microphone".to_owned(), "Headset".to_owned()];
        assert_eq!(
            select_input_device(Some("Headset"), &available),
            DeviceSelection::Requested(1)
        );
    }

    #[test]
    fn input_device_selection_falls_back_when_requested_name_is_missing() {
        let available = vec!["Dock microphone".to_owned()];
        assert_eq!(
            select_input_device(Some("Headset"), &available),
            DeviceSelection::FallbackDefault {
                requested: "Headset".to_owned()
            }
        );
    }

    #[test]
    fn input_device_selection_uses_default_without_a_request() {
        assert_eq!(
            select_input_device(None, &["Dock microphone".to_owned()]),
            DeviceSelection::Default
        );
    }

    #[test]
    fn audio_worker_delivers_one_flush_tail_after_resampled_batches() {
        let input = sine_wave(1_000.0, 48_000, WORKER_BATCH_SAMPLES * 3);
        let (mut producer, consumer) = RingBuffer::<f32>::new(input.len() + 1);
        for &sample in &input {
            producer.push(sample).expect("ring should accept sample");
        }
        drop(producer);

        let mut expected_resampler = AudioResampler::new(48_000, 16_000).expect("valid rates");
        let mut expected_batches = Vec::new();
        let mut batch = Vec::new();
        for chunk in input.chunks(WORKER_BATCH_SAMPLES) {
            expected_resampler.process_into(chunk, &mut batch);
            expected_batches.push(batch.clone());
        }
        expected_resampler.flush_into(&mut batch);
        expected_batches.push(batch.clone());
        assert!(!batch.is_empty(), "test input should produce a flush tail");

        let mut actual_batches = Vec::new();
        run_audio_worker(
            consumer,
            48_000,
            16_000,
            AudioResampler::new(48_000, 16_000).expect("valid rates"),
            &AtomicU64::new(0),
            |samples, _spectrum_analyzer| actual_batches.push(samples.to_vec()),
        );

        assert_eq!(actual_batches, expected_batches);
    }

    #[test]
    fn audio_worker_reports_session_total_when_samples_were_dropped() {
        let (producer, consumer) = RingBuffer::<f32>::new(4);
        drop(producer);

        let dropped_samples = Arc::new(AtomicU64::new(48));
        audio_worker(
            consumer,
            TEST_SAMPLE_RATE,
            TEST_SAMPLE_RATE,
            AudioResampler::new(TEST_SAMPLE_RATE, TEST_SAMPLE_RATE).expect("valid rates"),
            &dropped_samples,
            Arc::new(TestCaptureHandler::default()),
        );
    }

    #[test]
    fn dropped_duration_ms_converts_samples_at_input_rate() {
        assert_eq!(dropped_duration_ms(48, 48_000), 1);
    }

    #[test]
    fn same_rate_resampler_returns_input() {
        let mut resampler = AudioResampler::new(16_000, 16_000).expect("valid rates");
        let mut output = Vec::new();

        resampler.process_into(&[0.0, 0.5, 1.0], &mut output);

        assert_eq!(output, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn same_rate_resampler_supports_rates_above_u16() {
        let mut resampler = AudioResampler::new(96_000, 96_000).expect("valid rates");
        let mut output = Vec::new();

        resampler.process_into(&[0.0, 0.5, 1.0], &mut output);

        assert_eq!(output, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn resampler_preserves_audio_duration() {
        let input = vec![0.25; 48_000];
        let output = resample_all(&input, &[input.len()]);

        assert!(output.len().abs_diff(16_000) <= 1, "len={}", output.len());
    }

    #[test]
    fn resampler_output_is_independent_of_input_chunks() {
        let input = sine_wave(1_000.0, 48_000, 48_000);
        let single = resample_all(&input, &[input.len()]);
        let chunked = resample_all(&input, &[17_123, input.len() - 17_123]);

        assert_eq!(chunked, single);
    }

    #[test]
    fn resampler_attenuates_frequencies_above_output_nyquist() {
        let input = sine_wave(12_000.0, 48_000, 48_000);
        let output = resample_all(&input, &[input.len()]);
        let rms_ratio = rms(&output) / rms(&input);

        // Measured sherpa ratio: 0.003049. Linear interpolation folds this tone
        // to 4 kHz near full amplitude.
        assert!(rms_ratio < 0.05, "stopband RMS ratio was {rms_ratio:.6}");
    }

    fn resample_all(input: &[f32], chunks: &[usize]) -> Vec<f32> {
        let mut resampler = AudioResampler::new(48_000, 16_000).expect("valid rates");
        let mut output = Vec::new();
        let mut batch = Vec::new();
        let mut offset = 0;
        for &chunk_len in chunks {
            resampler.process_into(&input[offset..offset + chunk_len], &mut batch);
            output.extend_from_slice(&batch);
            offset += chunk_len;
        }
        assert_eq!(offset, input.len());
        resampler.flush_into(&mut batch);
        output.extend_from_slice(&batch);
        output
    }

    #[allow(clippy::cast_precision_loss)]
    fn sine_wave(frequency: f32, sample_rate: u32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| {
                let phase =
                    2.0 * std::f32::consts::PI * frequency * index as f32 / sample_rate as f32;
                phase.sin()
            })
            .collect()
    }

    #[allow(clippy::cast_precision_loss)]
    fn rms(samples: &[f32]) -> f32 {
        let mean_square =
            samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32;
        mean_square.sqrt()
    }
}
