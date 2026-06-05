use anyhow::Result;
use anyhow::anyhow;
use cpal::Device;
use cpal::FromSample;
use cpal::I24;
use cpal::Sample;
use cpal::SampleFormat;
use cpal::SizedSample;
use cpal::SupportedStreamConfig;
use cpal::SupportedStreamConfigRange;
use cpal::U24;
use cpal::traits::DeviceTrait;
use cpal::traits::HostTrait;
use cpal::traits::StreamTrait;

use crate::app::App;
use crate::app::SpectrumFrame;
use crate::dictation::DICTATION_SAMPLE_RATE;
use crate::dictation::DictationControl;
use crate::spectrum::SpectrumAnalyzer;

const SAMPLE_RATE: u32 = DICTATION_SAMPLE_RATE.as_hz();

pub(crate) fn capture(control: DictationControl, app: App) -> Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device found"))?;
    let config = input_config(&device)?;
    let stream = build_input_stream(&device, config, control, app)?;

    stream.play()?;

    Ok(stream)
}

fn input_config(device: &Device) -> Result<SupportedStreamConfig> {
    if let Some(config) = device
        .supported_input_configs()?
        .filter(|config| !config.sample_format().is_dsd())
        .filter_map(|config| config_for_sample_rate(config, SAMPLE_RATE))
        .min_by_key(input_config_score)
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

fn config_for_sample_rate(
    config: SupportedStreamConfigRange,
    sample_rate: u32,
) -> Option<SupportedStreamConfig> {
    let min = config.min_sample_rate();
    let max = config.max_sample_rate();

    if (min..=max).contains(&sample_rate) {
        Some(config.with_sample_rate(sample_rate))
    } else {
        None
    }
}

fn input_config_score(config: &SupportedStreamConfig) -> (u8, u16) {
    (
        sample_format_score(config.sample_format()),
        config.channels(),
    )
}

fn sample_format_score(format: SampleFormat) -> u8 {
    match format {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        SampleFormat::U16 => 2,
        SampleFormat::I24 => 3,
        SampleFormat::U24 => 4,
        SampleFormat::I32 => 5,
        SampleFormat::U32 => 6,
        SampleFormat::I8 => 7,
        SampleFormat::U8 => 8,
        SampleFormat::F64 => 9,
        SampleFormat::I64 => 10,
        SampleFormat::U64 => 11,
        _ => 12,
    }
}

fn build_input_stream(
    device: &Device,
    config: SupportedStreamConfig,
    control: DictationControl,
    app: App,
) -> Result<cpal::Stream> {
    eprintln!(
        "capturing microphone audio at {}Hz, {} channel(s), {}",
        config.sample_rate(),
        config.channels(),
        config.sample_format()
    );

    match config.sample_format() {
        SampleFormat::I8 => build_typed_input_stream::<i8>(device, config, control, app),
        SampleFormat::I16 => build_typed_input_stream::<i16>(device, config, control, app),
        SampleFormat::I24 => build_typed_input_stream::<I24>(device, config, control, app),
        SampleFormat::I32 => build_typed_input_stream::<i32>(device, config, control, app),
        SampleFormat::I64 => build_typed_input_stream::<i64>(device, config, control, app),
        SampleFormat::U8 => build_typed_input_stream::<u8>(device, config, control, app),
        SampleFormat::U16 => build_typed_input_stream::<u16>(device, config, control, app),
        SampleFormat::U24 => build_typed_input_stream::<U24>(device, config, control, app),
        SampleFormat::U32 => build_typed_input_stream::<u32>(device, config, control, app),
        SampleFormat::U64 => build_typed_input_stream::<u64>(device, config, control, app),
        SampleFormat::F32 => build_typed_input_stream::<f32>(device, config, control, app),
        SampleFormat::F64 => build_typed_input_stream::<f64>(device, config, control, app),
        format => Err(anyhow!("unsupported input sample format {format}")),
    }
}

fn build_typed_input_stream<T>(
    device: &Device,
    config: SupportedStreamConfig,
    control: DictationControl,
    app: App,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let stream_config = config.clone().into();
    let mut mic = MicInput::new(
        usize::from(config.channels()),
        config.sample_rate(),
        control,
        app,
    );

    Ok(device.build_input_stream(
        &stream_config,
        move |data: &[T], _| {
            mic.push(data);
        },
        |error| eprintln!("recording error: {error}"),
        None,
    )?)
}

struct MicInput {
    channels: usize,
    resampler: LinearResampler,
    spectrum_analyzer: SpectrumAnalyzer,
    control: DictationControl,
    app: App,
}

impl MicInput {
    fn new(channels: usize, input_sample_rate: u32, control: DictationControl, app: App) -> Self {
        Self {
            channels,
            resampler: LinearResampler::new(input_sample_rate, SAMPLE_RATE),
            spectrum_analyzer: SpectrumAnalyzer::new(SAMPLE_RATE),
            control,
            app,
        }
    }

    fn push<T>(&mut self, input: &[T])
    where
        T: Sample,
        f32: FromSample<T>,
    {
        let downmixed = input
            .chunks(self.channels)
            .map(|frame| {
                frame
                    .iter()
                    .map(|sample| f32::from_sample(*sample))
                    .sum::<f32>()
                    / frame.len() as f32
            })
            .collect::<Vec<_>>();
        let resampled = self.resampler.process(&downmixed);

        self.control.record_samples(&resampled);

        for sample in resampled {
            if let Some(bands) = self.spectrum_analyzer.push_sample(sample) {
                self.app.send_frame(SpectrumFrame::new(bands));
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

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }

        if self.input_sample_rate == self.output_sample_rate {
            return input.to_vec();
        }

        self.buffer.extend_from_slice(input);

        let ratio = self.input_sample_rate as f64 / self.output_sample_rate as f64;
        let mut output = Vec::with_capacity(
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
            self.buffer.drain(..consumed);
            self.input_position -= consumed as f64;
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_resampler_returns_input() {
        let mut resampler = LinearResampler::new(16_000, 16_000);

        assert_eq!(resampler.process(&[0.0, 0.5, 1.0]), vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn resampler_downsamples_linearly() {
        let mut resampler = LinearResampler::new(4, 2);

        assert_eq!(resampler.process(&[0.0, 1.0, 2.0, 3.0]), vec![0.0, 2.0]);
    }

    #[test]
    fn resampler_keeps_fractional_position_across_buffers() {
        let mut resampler = LinearResampler::new(2, 4);

        assert_eq!(resampler.process(&[0.0, 1.0]), vec![0.0, 0.5]);
        assert_eq!(resampler.process(&[2.0]), vec![1.0, 1.5]);
    }
}
