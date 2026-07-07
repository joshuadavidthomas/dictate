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
use cpal::U24;
use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::traits::StreamTrait;
use rtrb::Consumer;
use rtrb::Producer;
use rtrb::RingBuffer;

use crate::app::Overlay;
use crate::dictation::DICTATION_SAMPLE_RATE;
use crate::dictation::DictationControl;
use crate::dictation::RecordSamplesUpdate;
use crate::spectrum::SpectrumAnalyzer;
use crate::spectrum::SpectrumLevels;

const SAMPLE_RATE: u32 = DICTATION_SAMPLE_RATE.as_hz();
const AUDIO_RING_SAMPLES: usize = 192_000;
const WORKER_BATCH_SAMPLES: usize = 256;
const EMPTY_RING_SLEEP: Duration = Duration::from_millis(1);
const TARGET_CALLBACK_DURATION: Duration = Duration::from_millis(16);

pub(crate) struct Mic {
    stream: Option<cpal::Stream>,
    worker: Option<JoinHandle<()>>,
}

pub(crate) struct SpectrumMic {
    stream: Option<cpal::Stream>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Mic {
    fn drop(&mut self) {
        drop(self.stream.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for SpectrumMic {
    fn drop(&mut self) {
        drop(self.stream.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn capture(dictation: DictationControl, overlay: Overlay) -> Result<Mic> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device found"))?;
    let config = input_config(&device)?;
    let stream_config = stream_config_with_target_buffer(&config);
    let requested_fixed_buffer = matches!(stream_config.buffer_size, BufferSize::Fixed(_));

    match capture_with_config(
        &device,
        &config,
        stream_config,
        dictation.clone(),
        overlay.clone(),
    ) {
        Ok(mic) => Ok(mic),
        Err(error) if requested_fixed_buffer => {
            eprintln!("fixed input buffer size rejected: {error:#}; falling back to default");
            capture_with_config(&device, &config, config.config(), dictation, overlay)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn capture_spectrum(levels: SpectrumLevels) -> Result<SpectrumMic> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device found"))?;
    let config = input_config(&device)?;
    let stream_config = stream_config_with_target_buffer(&config);
    let requested_fixed_buffer = matches!(stream_config.buffer_size, BufferSize::Fixed(_));

    match capture_spectrum_with_config(&device, &config, stream_config, levels.clone()) {
        Ok(mic) => Ok(mic),
        Err(error) if requested_fixed_buffer => {
            eprintln!("fixed input buffer size rejected: {error:#}; falling back to default");
            capture_spectrum_with_config(&device, &config, config.config(), levels)
        }
        Err(error) => Err(error),
    }
}

fn capture_with_config(
    device: &Device,
    supported_config: &SupportedStreamConfig,
    stream_config: StreamConfig,
    dictation: DictationControl,
    overlay: Overlay,
) -> Result<Mic> {
    let input_sample_rate = stream_config.sample_rate;
    let (producer, consumer) = RingBuffer::<f32>::new(AUDIO_RING_SAMPLES);
    let dropped_samples = Arc::new(AtomicU64::new(0));
    let stream_error = StreamErrorHandler::new(dictation.clone(), overlay.clone());

    eprintln!(
        "capturing microphone audio at {}Hz, {} channel(s), {}, {:?} buffer",
        stream_config.sample_rate,
        stream_config.channels,
        supported_config.sample_format(),
        stream_config.buffer_size
    );

    let stream = build_input_stream_for_format(
        device,
        supported_config,
        stream_config,
        producer,
        dropped_samples.clone(),
        move |error| stream_error.handle(error),
    )?;

    stream.play()?;
    let worker = thread::spawn(move || {
        audio_worker(
            consumer,
            input_sample_rate,
            dropped_samples,
            dictation,
            Some(overlay),
        )
    });

    Ok(Mic {
        stream: Some(stream),
        worker: Some(worker),
    })
}

fn capture_spectrum_with_config(
    device: &Device,
    supported_config: &SupportedStreamConfig,
    stream_config: StreamConfig,
    levels: SpectrumLevels,
) -> Result<SpectrumMic> {
    let input_sample_rate = stream_config.sample_rate;
    let (producer, consumer) = RingBuffer::<f32>::new(AUDIO_RING_SAMPLES);
    let dropped_samples = Arc::new(AtomicU64::new(0));

    eprintln!(
        "capturing microphone spectrum at {}Hz, {} channel(s), {}, {:?} buffer",
        stream_config.sample_rate,
        stream_config.channels,
        supported_config.sample_format(),
        stream_config.buffer_size
    );

    let stream = build_input_stream_for_format(
        device,
        supported_config,
        stream_config,
        producer,
        dropped_samples.clone(),
        |error| eprintln!("spectrum recording error: {error}"),
    )?;

    stream.play()?;
    let worker = thread::spawn(move || {
        spectrum_audio_worker(consumer, input_sample_rate, dropped_samples, levels)
    });

    Ok(SpectrumMic {
        stream: Some(stream),
        worker: Some(worker),
    })
}

fn stream_config_with_target_buffer(config: &SupportedStreamConfig) -> StreamConfig {
    let mut stream_config = config.config();
    let target_frames = (config.sample_rate() as f32 * TARGET_CALLBACK_DURATION.as_secs_f32())
        .round()
        .max(1.0) as u32;
    stream_config.buffer_size = match *config.buffer_size() {
        SupportedBufferSize::Range { min, max } => BufferSize::Fixed(target_frames.clamp(min, max)),
        SupportedBufferSize::Unknown => BufferSize::Fixed(target_frames),
    };
    stream_config
}

fn input_config(device: &Device) -> Result<SupportedStreamConfig> {
    for config in device.supported_input_configs()? {
        if config.sample_format().is_dsd() {
            continue;
        }

        if (config.min_sample_rate()..=config.max_sample_rate()).contains(&SAMPLE_RATE) {
            return Ok(config.with_sample_rate(SAMPLE_RATE));
        }
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

struct StreamErrorHandler {
    dictation: DictationControl,
    overlay: Overlay,
}

impl StreamErrorHandler {
    fn new(dictation: DictationControl, overlay: Overlay) -> Self {
        Self { dictation, overlay }
    }

    fn handle(&self, error: cpal::StreamError) {
        eprintln!("recording error: {error}");
        if self.dictation.abort_recording() {
            self.overlay.hide();
        }
    }
}

fn build_input_stream_for_format<E>(
    device: &Device,
    supported_config: &SupportedStreamConfig,
    stream_config: StreamConfig,
    producer: Producer<f32>,
    dropped_samples: Arc<AtomicU64>,
    stream_error: E,
) -> Result<cpal::Stream>
where
    E: FnMut(cpal::StreamError) + Send + 'static,
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
        format => Err(anyhow!("unsupported input sample format {format}")),
    }
}

fn build_input_stream<T, E>(
    device: &Device,
    stream_config: StreamConfig,
    mut producer: Producer<f32>,
    dropped_samples: Arc<AtomicU64>,
    stream_error: E,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
    E: FnMut(cpal::StreamError) + Send + 'static,
{
    let channels = usize::from(stream_config.channels);

    Ok(device.build_input_stream(
        &stream_config,
        move |data: &[T], _| {
            for frame in data.chunks(channels) {
                let sample = frame
                    .iter()
                    .map(|sample| f32::from_sample(*sample))
                    .sum::<f32>()
                    / frame.len() as f32;
                if producer.push(sample).is_err() {
                    dropped_samples.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        stream_error,
        None,
    )?)
}

fn audio_worker(
    consumer: Consumer<f32>,
    input_sample_rate: u32,
    dropped_samples: Arc<AtomicU64>,
    dictation: DictationControl,
    overlay: Option<Overlay>,
) {
    run_audio_worker(
        consumer,
        input_sample_rate,
        dropped_samples,
        |samples, spectrum_analyzer| {
            if let RecordSamplesUpdate::AutoStopped { duration } = dictation.record_samples(samples)
            {
                if let Some(overlay) = &overlay {
                    overlay.hide();
                }
                eprintln!(
                    "dictation reached the {} s limit; transcribing captured audio",
                    duration.as_secs()
                );
            }

            for &sample in samples {
                if let Some(bands) = spectrum_analyzer.push_sample(sample)
                    && let Some(overlay) = &overlay
                {
                    overlay.send_spectrum(bands);
                }
            }
        },
    );
}

fn spectrum_audio_worker(
    consumer: Consumer<f32>,
    input_sample_rate: u32,
    dropped_samples: Arc<AtomicU64>,
    levels: SpectrumLevels,
) {
    run_audio_worker(
        consumer,
        input_sample_rate,
        dropped_samples,
        move |samples, spectrum_analyzer| {
            for &sample in samples {
                if let Some(bands) = spectrum_analyzer.push_sample(sample) {
                    levels.set(bands);
                }
            }
        },
    );
}

fn run_audio_worker(
    mut consumer: Consumer<f32>,
    input_sample_rate: u32,
    dropped_samples: Arc<AtomicU64>,
    mut sink: impl FnMut(&[f32], &mut SpectrumAnalyzer),
) {
    let mut overflow_warned = false;
    let mut input = Vec::with_capacity(WORKER_BATCH_SAMPLES);
    let mut samples = Vec::with_capacity(WORKER_BATCH_SAMPLES);
    let mut resampler = LinearResampler::new(input_sample_rate, SAMPLE_RATE);
    let mut spectrum_analyzer = SpectrumAnalyzer::new(SAMPLE_RATE);

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
            warn_on_first_overflow(&dropped_samples, input_sample_rate, &mut overflow_warned);
            thread::sleep(EMPTY_RING_SLEEP);
            continue;
        }

        resampler.process_into(&input, &mut samples);
        sink(&samples, &mut spectrum_analyzer);

        warn_on_first_overflow(&dropped_samples, input_sample_rate, &mut overflow_warned);
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

struct LinearResampler {
    input_sample_rate: u32,
    output_sample_rate: u32,
    input_position: f64,
    buffer: Vec<f32>,
}

impl LinearResampler {
    fn new(input_sample_rate: u32, output_sample_rate: u32) -> Self {
        Self {
            input_sample_rate,
            output_sample_rate,
            input_position: 0.0,
            buffer: Vec::new(),
        }
    }

    fn process_into(&mut self, input: &[f32], output: &mut Vec<f32>) {
        output.clear();

        if input.is_empty() {
            return;
        }

        if self.input_sample_rate == self.output_sample_rate {
            output.extend_from_slice(input);
            return;
        }

        self.buffer.extend_from_slice(input);

        let ratio = self.input_sample_rate as f64 / self.output_sample_rate as f64;
        output.reserve(
            (input.len() as f64 * self.output_sample_rate as f64 / self.input_sample_rate as f64)
                .ceil() as usize,
        );

        while self.input_position + 1.0 < self.buffer.len() as f64 {
            let index = self.input_position.floor() as usize;
            let fraction = (self.input_position - index as f64) as f32;
            let current = self.buffer[index];
            let next = self.buffer[index + 1];
            output.push(current + (next - current) * fraction);
            self.input_position += ratio;
        }

        let consumed = self.input_position.floor() as usize;
        if consumed > 0 {
            let remaining = self.buffer.len() - consumed;
            self.buffer.copy_within(consumed.., 0);
            self.buffer.truncate(remaining);
            self.input_position -= consumed as f64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_worker_exits_when_producer_is_dropped() {
        let (producer, consumer) = RingBuffer::<f32>::new(4);
        drop(producer);

        audio_worker(
            consumer,
            SAMPLE_RATE,
            Arc::new(AtomicU64::new(0)),
            DictationControl::new(),
            None,
        );
    }

    #[test]
    fn audio_worker_reports_session_total_when_samples_were_dropped() {
        let (producer, consumer) = RingBuffer::<f32>::new(4);
        drop(producer);

        audio_worker(
            consumer,
            SAMPLE_RATE,
            Arc::new(AtomicU64::new(48)),
            DictationControl::new(),
            None,
        );
    }

    #[test]
    fn dropped_duration_ms_converts_samples_at_input_rate() {
        assert_eq!(dropped_duration_ms(48, 48_000), 1);
    }

    #[test]
    fn same_rate_resampler_returns_input() {
        let mut resampler = LinearResampler::new(16_000, 16_000);
        let mut output = Vec::new();

        resampler.process_into(&[0.0, 0.5, 1.0], &mut output);

        assert_eq!(output, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn resampler_downsamples_linearly() {
        let mut resampler = LinearResampler::new(4, 2);
        let mut output = Vec::new();

        resampler.process_into(&[0.0, 1.0, 2.0, 3.0], &mut output);

        assert_eq!(output, vec![0.0, 2.0]);
    }

    #[test]
    fn resampler_keeps_fractional_position_across_buffers() {
        let mut resampler = LinearResampler::new(2, 4);
        let mut output = Vec::new();

        resampler.process_into(&[0.0, 1.0], &mut output);
        assert_eq!(output, vec![0.0, 0.5]);

        resampler.process_into(&[2.0], &mut output);
        assert_eq!(output, vec![1.0, 1.5]);
    }
}
