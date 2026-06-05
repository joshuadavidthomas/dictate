use std::fs::File;
use std::fs::{
    self,
};
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::thread::{
    self,
};
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use bzip2::read::BzDecoder;
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
use directories::ProjectDirs;
use sherpa_onnx::OfflineRecognizer;
use sherpa_onnx::SileroVadModelConfig;
use sherpa_onnx::VadModelConfig;
use sherpa_onnx::VoiceActivityDetector;
use tar::Archive;

use crate::models::ModelCatalogEntry;
use crate::models::VadModel;
use crate::models::default_model;
use crate::spectrum::SpectrumAnalyzer;
use crate::state::SpectrumLevels;

const SAMPLE_RATE: u32 = 16_000;
const VAD_WINDOW_SIZE: usize = 512;
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MIN_TRANSCRIBED_SEGMENT_DURATION: Duration = Duration::from_millis(400);
const MIN_TRANSCRIBED_SEGMENT_RMS: f32 = 0.01;

pub struct ConsoleTranscriber {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ConsoleTranscriber {
    pub fn start(spectrum: SpectrumLevels) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            if let Err(error) = run(thread_stop, spectrum) {
                eprintln!("transcription failed: {error:#}");
            }
        });

        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for ConsoleTranscriber {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(stop: Arc<AtomicBool>, spectrum: SpectrumLevels) -> Result<()> {
    ensure_models_downloaded()?;

    let model = transcription_model();
    let recognizer = model.create_recognizer(&transcription_model_dir()?)?;
    let vad = create_vad()?;
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device found"))?;
    let config = input_config(&device)?;
    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let stream = build_input_stream(&device, config, samples.clone(), stop.clone(), spectrum)?;

    stream.play()?;
    eprintln!("transcribing microphone audio; close the overlay to stop");

    while !stop.load(Ordering::Acquire) {
        thread::sleep(POLL_INTERVAL);

        loop {
            let window = {
                let mut samples = samples.lock().unwrap();
                if samples.len() < VAD_WINDOW_SIZE {
                    break;
                }

                samples.drain(..VAD_WINDOW_SIZE).collect::<Vec<_>>()
            };

            vad.accept_waveform(&window);
            transcribe_ready_segments(&recognizer, &vad);
        }
    }

    vad.flush();
    transcribe_ready_segments(&recognizer, &vad);

    Ok(())
}

fn transcribe_ready_segments(recognizer: &OfflineRecognizer, vad: &VoiceActivityDetector) {
    while !vad.is_empty() {
        let Some(segment) = vad.front() else {
            break;
        };

        let samples = segment.samples().to_vec();
        vad.pop();

        if !should_transcribe_segment(&samples) {
            continue;
        }

        let stream = recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, &samples);
        recognizer.decode(&stream);

        let Some(result) = stream.get_result() else {
            continue;
        };

        let text = result.text.trim();
        if should_print_transcript(text) {
            println!("{text}");
        }
    }
}

fn should_transcribe_segment(samples: &[f32]) -> bool {
    let duration = Duration::from_secs_f32(samples.len() as f32 / SAMPLE_RATE as f32);
    duration >= MIN_TRANSCRIBED_SEGMENT_DURATION && rms(samples) >= MIN_TRANSCRIBED_SEGMENT_RMS
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_squares: f32 = samples.iter().map(|sample| sample * sample).sum();
    (sum_squares / samples.len() as f32).sqrt()
}

fn should_print_transcript(text: &str) -> bool {
    if text.is_empty() || is_repeated_punctuation(text) {
        return false;
    }

    let stripped = text.trim_matches(['(', ')']).trim();

    !matches!(
        stripped.to_ascii_lowercase().as_str(),
        "cough" | "coughing" | "static" | "phone buzz" | "buzz" | "noise" | "music" | "laughter"
    )
}

fn is_repeated_punctuation(text: &str) -> bool {
    let mut chars = text.chars().filter(|character| !character.is_whitespace());
    let Some(first) = chars.next() else {
        return true;
    };

    first.is_ascii_punctuation() && chars.all(|character| character == first)
}

fn create_vad() -> Result<VoiceActivityDetector> {
    let silero_config = SileroVadModelConfig {
        model: Some(vad_model_path()?.to_string_lossy().to_string()),
        threshold: 0.5,
        min_silence_duration: 0.35,
        min_speech_duration: 0.25,
        window_size: VAD_WINDOW_SIZE as i32,
        max_speech_duration: 20.0,
    };

    let config = VadModelConfig {
        silero_vad: silero_config,
        sample_rate: SAMPLE_RATE as i32,
        num_threads: 1,
        provider: Some("cpu".to_string()),
        debug: false,
        ten_vad: Default::default(),
    };

    VoiceActivityDetector::create(&config, 30.0)
        .ok_or_else(|| anyhow!("failed to create sherpa-onnx Silero VAD"))
}

fn ensure_models_downloaded() -> Result<()> {
    ensure_transcription_model_downloaded()?;
    ensure_silero_vad_downloaded()?;
    Ok(())
}

fn ensure_transcription_model_downloaded() -> Result<()> {
    let model_dir = transcription_model_dir()?;
    if model_dir.exists() {
        return Ok(());
    }

    let model = transcription_model();
    let models_dir = models_dir()?;
    fs::create_dir_all(&models_dir)?;
    let archive_path = models_dir.join(model.archive_name());
    let download_url = model.download_url();

    eprintln!("downloading {}...", model.display_name());
    download_file(&download_url, &archive_path)?;

    eprintln!("extracting {}...", model.display_name());
    extract_tar_bz2(&archive_path, model.id().as_str())?;

    fs::remove_file(&archive_path).ok();
    eprintln!("{} ready", model.display_name());

    Ok(())
}

fn ensure_silero_vad_downloaded() -> Result<()> {
    let vad_path = vad_model_path()?;
    if vad_path.exists() {
        return Ok(());
    }

    fs::create_dir_all(models_dir()?)?;
    eprintln!("downloading {}...", VadModel::display_name());
    download_file(VadModel::download_url(), &vad_path)?;
    eprintln!("{} ready", VadModel::display_name());

    Ok(())
}

fn download_file(url: &str, output_path: &Path) -> Result<()> {
    let response = ureq::get(url)
        .call()
        .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
    let total = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = response.into_reader();
    let mut file = File::create(output_path)?;
    let mut buffer = [0_u8; 1024 * 1024];
    let mut downloaded = 0_u64;
    let mut next_report = 0_u64;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        file.write_all(&buffer[..read])?;
        downloaded += read as u64;

        if total > 0 && downloaded >= next_report {
            eprintln!(
                "downloaded {}/{} MB",
                downloaded / 1_000_000,
                total / 1_000_000
            );
            next_report = downloaded + 25_000_000;
        }
    }

    Ok(())
}

fn extract_tar_bz2(archive_path: &Path, model_name: &str) -> Result<()> {
    let models_dir = models_dir()?;
    let temp_extract_dir = models_dir.join(format!("{model_name}.extracting"));
    let final_model_dir = models_dir.join(model_name);

    if temp_extract_dir.exists() {
        fs::remove_dir_all(&temp_extract_dir)?;
    }
    fs::create_dir_all(&temp_extract_dir)?;

    let tar_bz2 = File::open(archive_path)?;
    let tar = BzDecoder::new(tar_bz2);
    let mut archive = Archive::new(tar);
    archive.unpack(&temp_extract_dir)?;

    let extracted_dirs = fs::read_dir(&temp_extract_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_type()
                .map(|file_type| file_type.is_dir())
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    if final_model_dir.exists() {
        fs::remove_dir_all(&final_model_dir)?;
    }

    if extracted_dirs.len() == 1 {
        fs::rename(extracted_dirs[0].path(), &final_model_dir)?;
        fs::remove_dir_all(&temp_extract_dir)?;
    } else {
        fs::rename(&temp_extract_dir, &final_model_dir)?;
    }

    Ok(())
}

fn transcription_model() -> &'static ModelCatalogEntry {
    default_model()
}

fn transcription_model_dir() -> Result<PathBuf> {
    Ok(transcription_model().local_dir(&models_dir()?))
}

fn vad_model_path() -> Result<PathBuf> {
    Ok(VadModel::local_path(&models_dir()?))
}

fn models_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "dictate")
        .ok_or_else(|| anyhow!("could not determine dictate data directory"))?;
    Ok(dirs.data_dir().join("models"))
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
    samples: Arc<Mutex<Vec<f32>>>,
    stop: Arc<AtomicBool>,
    spectrum: SpectrumLevels,
) -> Result<cpal::Stream> {
    eprintln!(
        "capturing microphone audio at {}Hz, {} channel(s), {}",
        config.sample_rate(),
        config.channels(),
        config.sample_format()
    );

    match config.sample_format() {
        SampleFormat::I8 => build_typed_input_stream::<i8>(device, config, samples, stop, spectrum),
        SampleFormat::I16 => {
            build_typed_input_stream::<i16>(device, config, samples, stop, spectrum)
        }
        SampleFormat::I24 => {
            build_typed_input_stream::<I24>(device, config, samples, stop, spectrum)
        }
        SampleFormat::I32 => {
            build_typed_input_stream::<i32>(device, config, samples, stop, spectrum)
        }
        SampleFormat::I64 => {
            build_typed_input_stream::<i64>(device, config, samples, stop, spectrum)
        }
        SampleFormat::U8 => build_typed_input_stream::<u8>(device, config, samples, stop, spectrum),
        SampleFormat::U16 => {
            build_typed_input_stream::<u16>(device, config, samples, stop, spectrum)
        }
        SampleFormat::U24 => {
            build_typed_input_stream::<U24>(device, config, samples, stop, spectrum)
        }
        SampleFormat::U32 => {
            build_typed_input_stream::<u32>(device, config, samples, stop, spectrum)
        }
        SampleFormat::U64 => {
            build_typed_input_stream::<u64>(device, config, samples, stop, spectrum)
        }
        SampleFormat::F32 => {
            build_typed_input_stream::<f32>(device, config, samples, stop, spectrum)
        }
        SampleFormat::F64 => {
            build_typed_input_stream::<f64>(device, config, samples, stop, spectrum)
        }
        format => Err(anyhow!("unsupported input sample format {format}")),
    }
}

fn build_typed_input_stream<T>(
    device: &Device,
    config: SupportedStreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    stop: Arc<AtomicBool>,
    spectrum: SpectrumLevels,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let stream_config = config.clone().into();
    let mut processor = AudioProcessor::new(
        usize::from(config.channels()),
        config.sample_rate(),
        samples,
        spectrum,
    );

    Ok(device.build_input_stream(
        &stream_config,
        move |data: &[T], _| {
            if stop.load(Ordering::Acquire) {
                return;
            }

            processor.process(data);
        },
        |error| eprintln!("recording error: {error}"),
        None,
    )?)
}

struct AudioProcessor {
    channels: usize,
    resampler: LinearResampler,
    spectrum_analyzer: SpectrumAnalyzer,
    samples: Arc<Mutex<Vec<f32>>>,
    spectrum: SpectrumLevels,
}

impl AudioProcessor {
    fn new(
        channels: usize,
        input_sample_rate: u32,
        samples: Arc<Mutex<Vec<f32>>>,
        spectrum: SpectrumLevels,
    ) -> Self {
        Self {
            channels,
            resampler: LinearResampler::new(input_sample_rate, SAMPLE_RATE),
            spectrum_analyzer: SpectrumAnalyzer::new(SAMPLE_RATE),
            samples,
            spectrum,
        }
    }

    fn process<T>(&mut self, input: &[T])
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

        if let Ok(mut samples) = self.samples.lock() {
            samples.extend_from_slice(&resampled);
        }

        for sample in resampled {
            if let Some(bands) = self.spectrum_analyzer.push_sample(sample) {
                self.spectrum.set(bands);
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
