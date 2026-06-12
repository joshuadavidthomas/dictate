use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use cpal::Device;
use cpal::FromSample;
use cpal::I24;
use cpal::Sample;
use cpal::SampleFormat;
use cpal::SizedSample;
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
use crate::spectrum::SpectrumAnalyzer;

const SAMPLE_RATE: u32 = DICTATION_SAMPLE_RATE.as_hz();
const AUDIO_RING_SAMPLES: usize = 192_000;
const WORKER_BATCH_SAMPLES: usize = 256;
const EMPTY_RING_SLEEP: Duration = Duration::from_millis(1);

pub(crate) struct Mic {
    _stream: cpal::Stream,
    _worker: JoinHandle<()>,
}

pub(crate) fn capture(dictation: DictationControl, overlay: Overlay) -> Result<Mic> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device found"))?;
    let config = input_config(&device)?;
    let input_sample_rate = config.sample_rate();
    let (producer, consumer) = RingBuffer::<f32>::new(AUDIO_RING_SAMPLES);
    let stream_error = StreamErrorHandler::new(dictation.clone(), overlay.clone());

    eprintln!(
        "capturing microphone audio at {}Hz, {} channel(s), {}",
        config.sample_rate(),
        config.channels(),
        config.sample_format()
    );

    let stream = match config.sample_format() {
        SampleFormat::I8 => build_input_stream::<i8>(&device, config, producer, stream_error),
        SampleFormat::I16 => build_input_stream::<i16>(&device, config, producer, stream_error),
        SampleFormat::I24 => build_input_stream::<I24>(&device, config, producer, stream_error),
        SampleFormat::I32 => build_input_stream::<i32>(&device, config, producer, stream_error),
        SampleFormat::I64 => build_input_stream::<i64>(&device, config, producer, stream_error),
        SampleFormat::U8 => build_input_stream::<u8>(&device, config, producer, stream_error),
        SampleFormat::U16 => build_input_stream::<u16>(&device, config, producer, stream_error),
        SampleFormat::U24 => build_input_stream::<U24>(&device, config, producer, stream_error),
        SampleFormat::U32 => build_input_stream::<u32>(&device, config, producer, stream_error),
        SampleFormat::U64 => build_input_stream::<u64>(&device, config, producer, stream_error),
        SampleFormat::F32 => build_input_stream::<f32>(&device, config, producer, stream_error),
        SampleFormat::F64 => build_input_stream::<f64>(&device, config, producer, stream_error),
        format => Err(anyhow!("unsupported input sample format {format}")),
    }?;

    stream.play()?;
    let worker =
        thread::spawn(move || audio_worker(consumer, input_sample_rate, dictation, overlay));

    Ok(Mic {
        _stream: stream,
        _worker: worker,
    })
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
        self.dictation.mark_unavailable();
        self.overlay.hide();
    }
}

fn build_input_stream<T>(
    device: &Device,
    config: SupportedStreamConfig,
    mut producer: Producer<f32>,
    stream_error: StreamErrorHandler,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels());
    let stream_config = config.into();

    Ok(device.build_input_stream(
        &stream_config,
        move |data: &[T], _| {
            for frame in data.chunks(channels) {
                let sample = frame
                    .iter()
                    .map(|sample| f32::from_sample(*sample))
                    .sum::<f32>()
                    / frame.len() as f32;
                let _ = producer.push(sample);
            }
        },
        move |error| stream_error.handle(error),
        None,
    )?)
}

fn audio_worker(
    mut consumer: Consumer<f32>,
    input_sample_rate: u32,
    dictation: DictationControl,
    overlay: Overlay,
) {
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
            thread::sleep(EMPTY_RING_SLEEP);
            continue;
        }

        resampler.process_into(&input, &mut samples);
        dictation.record_samples(&samples);

        for &sample in &samples {
            if let Some(bands) = spectrum_analyzer.push_sample(sample) {
                overlay.send_spectrum(bands);
            }
        }
    }
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
