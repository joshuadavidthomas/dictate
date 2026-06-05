use std::collections::VecDeque;
use std::fs;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;
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
use tar::Archive;

use crate::app::App;
use crate::app::SpectrumFrame;
use crate::dictation::CapturedUtterance;
use crate::dictation::DICTATION_SAMPLE_RATE;
use crate::dictation::DictationPhase;
use crate::dictation::DictationSession;
use crate::models::ModelCatalogEntry;
use crate::models::default_model;
use crate::processing::DictationContext;
use crate::processing::PostProcessor;
use crate::processing::RawTranscript;
use crate::spectrum::SpectrumAnalyzer;

const SAMPLE_RATE: u32 = DICTATION_SAMPLE_RATE.as_hz();
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MIN_DICTATION_DURATION: Duration = Duration::from_millis(400);
const MIN_DICTATION_RMS: f32 = 0.01;

pub struct DictationTranscriber {
    control: DictationControl,
    app: App,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DictationTranscriber {
    pub fn start(app: App) -> Self {
        let control = DictationControl::new();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_control = control.clone();
        let thread_app = app.clone();
        let run_control = thread_control.clone();
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            if let Err(error) = run(thread_stop, run_control, thread_app) {
                eprintln!("transcription failed: {error:#}");
                thread_control.mark_unavailable();
            }
        });

        Self {
            control,
            app,
            stop,
            thread: Some(thread),
        }
    }

    pub fn start_recording(&self) -> DictationPhase {
        self.report_outcome(self.control.start_recording());
        self.phase()
    }

    pub fn stop_recording(&self) -> DictationPhase {
        self.report_outcome(self.control.stop_recording());
        let phase = self.phase();
        if phase == DictationPhase::Idle {
            self.app.hide();
        }
        phase
    }

    pub fn toggle(&self) -> DictationPhase {
        match self.phase() {
            DictationPhase::Idle => self.start_recording(),
            DictationPhase::Recording => self.stop_recording(),
            DictationPhase::Transcribing | DictationPhase::Unavailable => {
                self.report_outcome(ControlOutcome::Busy(self.phase()));
                self.phase()
            }
        }
    }

    pub fn cancel_recording(&self) -> DictationPhase {
        self.report_outcome(self.control.cancel_recording());
        self.phase()
    }

    pub fn phase(&self) -> DictationPhase {
        self.control.phase()
    }

    fn report_outcome(&self, outcome: ControlOutcome) {
        match outcome {
            ControlOutcome::Started => {
                self.app.show();
                eprintln!("dictation started; run `dictate record stop` to transcribe")
            }
            ControlOutcome::Stopped => eprintln!("dictation stopped; transcribing captured audio"),
            ControlOutcome::Cancelled => {
                self.app.hide();
                eprintln!("dictation cancelled")
            }
            ControlOutcome::Ignored(reason) => eprintln!("record command ignored: {reason}"),
            ControlOutcome::Busy(phase) => {
                eprintln!("cannot change recording while {}", phase.label())
            }
        }
    }
}

impl Drop for DictationTranscriber {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone)]
struct DictationControl {
    state: Arc<Mutex<DictationControlState>>,
}

impl DictationControl {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DictationControlState::new())),
        }
    }

    fn start_recording(&self) -> ControlOutcome {
        let mut state = self.state.lock().unwrap();

        match state.phase {
            DictationPhase::Idle => {
                state.active_session = Some(DictationSession::new(DICTATION_SAMPLE_RATE));
                state.phase = DictationPhase::Recording;
                ControlOutcome::Started
            }
            DictationPhase::Recording => ControlOutcome::Ignored("already recording"),
            DictationPhase::Transcribing => ControlOutcome::Busy(DictationPhase::Transcribing),
            DictationPhase::Unavailable => ControlOutcome::Busy(DictationPhase::Unavailable),
        }
    }

    fn stop_recording(&self) -> ControlOutcome {
        let mut state = self.state.lock().unwrap();

        match state.phase {
            DictationPhase::Idle => ControlOutcome::Ignored("not recording"),
            DictationPhase::Recording => {
                let utterance = state
                    .active_session
                    .take()
                    .and_then(DictationSession::finish);
                if let Some(utterance) = utterance {
                    state.ready_utterances.push_back(utterance);
                    state.phase = DictationPhase::Transcribing;
                } else {
                    state.phase = DictationPhase::Idle;
                }
                ControlOutcome::Stopped
            }
            DictationPhase::Transcribing => ControlOutcome::Busy(DictationPhase::Transcribing),
            DictationPhase::Unavailable => ControlOutcome::Busy(DictationPhase::Unavailable),
        }
    }

    fn cancel_recording(&self) -> ControlOutcome {
        let mut state = self.state.lock().unwrap();

        match state.phase {
            DictationPhase::Idle => ControlOutcome::Ignored("not recording"),
            DictationPhase::Recording => {
                state.active_session = None;
                state.ready_utterances.clear();
                state.phase = DictationPhase::Idle;
                ControlOutcome::Cancelled
            }
            DictationPhase::Transcribing => ControlOutcome::Busy(DictationPhase::Transcribing),
            DictationPhase::Unavailable => ControlOutcome::Busy(DictationPhase::Unavailable),
        }
    }

    fn phase(&self) -> DictationPhase {
        self.state.lock().unwrap().phase
    }

    fn record_samples(&self, samples: &[f32]) {
        let mut state = self.state.lock().unwrap();
        if let Some(session) = state.active_session.as_mut() {
            session.push_samples(samples);
        }
    }

    fn take_utterance(&self) -> Option<CapturedUtterance> {
        self.state.lock().unwrap().ready_utterances.pop_front()
    }

    fn finish_transcription(&self) {
        let mut state = self.state.lock().unwrap();
        if state.ready_utterances.is_empty() && state.active_session.is_none() {
            state.phase = DictationPhase::Idle;
        }
    }

    fn mark_unavailable(&self) {
        let mut state = self.state.lock().unwrap();
        state.phase = DictationPhase::Unavailable;
        state.active_session = None;
        state.ready_utterances.clear();
    }
}

#[derive(Debug)]
struct DictationControlState {
    phase: DictationPhase,
    active_session: Option<DictationSession>,
    ready_utterances: VecDeque<CapturedUtterance>,
}

impl DictationControlState {
    fn new() -> Self {
        Self {
            phase: DictationPhase::Idle,
            active_session: None,
            ready_utterances: VecDeque::new(),
        }
    }
}

enum ControlOutcome {
    Started,
    Stopped,
    Cancelled,
    Ignored(&'static str),
    Busy(DictationPhase),
}

fn run(stop: Arc<AtomicBool>, control: DictationControl, app: App) -> Result<()> {
    ensure_transcription_model_downloaded()?;

    let model = transcription_model();
    let recognizer = model.create_recognizer(&transcription_model_dir()?)?;
    let post_processor = PostProcessor;
    let dictation_context = DictationContext::default();
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow!("no default input device found"))?;
    let config = input_config(&device)?;
    let stream = build_input_stream(&device, config, control.clone(), stop.clone(), app.clone())?;

    stream.play()?;
    eprintln!("microphone ready; run `dictate record start` to start dictation");

    while !stop.load(Ordering::Acquire) {
        thread::sleep(POLL_INTERVAL);

        let Some(utterance) = control.take_utterance() else {
            continue;
        };

        transcribe_and_print_utterance(
            &recognizer,
            &post_processor,
            &dictation_context,
            &utterance,
        );
        control.finish_transcription();
        app.hide();
    }

    Ok(())
}

fn transcribe_and_print_utterance(
    recognizer: &OfflineRecognizer,
    post_processor: &PostProcessor,
    dictation_context: &DictationContext,
    utterance: &CapturedUtterance,
) {
    if !should_transcribe_utterance(utterance) {
        eprintln!("captured dictation was too short or too quiet");
        return;
    }

    let Some(raw) = transcribe_utterance(recognizer, utterance) else {
        return;
    };

    if !should_print_transcript(raw.as_str()) {
        return;
    }

    let processed = post_processor.process(raw, dictation_context);
    if !processed.is_empty() {
        println!("{}", processed.as_str());
    }
}

fn transcribe_utterance(
    recognizer: &OfflineRecognizer,
    utterance: &CapturedUtterance,
) -> Option<RawTranscript> {
    let stream = recognizer.create_stream();
    stream.accept_waveform(utterance.sample_rate().as_hz() as i32, utterance.samples());
    recognizer.decode(&stream);

    let result = stream.get_result()?;
    let text = result.text.trim();
    if text.is_empty() {
        None
    } else {
        Some(RawTranscript::new(text))
    }
}

fn should_transcribe_utterance(utterance: &CapturedUtterance) -> bool {
    utterance.duration() >= MIN_DICTATION_DURATION && rms(utterance.samples()) >= MIN_DICTATION_RMS
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

fn download_file(url: &str, output_path: &Path) -> Result<()> {
    let mut response = ureq::get(url)
        .call()
        .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
    let total = response.body().content_length().unwrap_or(0);
    let mut reader = response.body_mut().as_reader();
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
    control: DictationControl,
    stop: Arc<AtomicBool>,
    app: App,
) -> Result<cpal::Stream> {
    eprintln!(
        "capturing microphone audio at {}Hz, {} channel(s), {}",
        config.sample_rate(),
        config.channels(),
        config.sample_format()
    );

    match config.sample_format() {
        SampleFormat::I8 => build_typed_input_stream::<i8>(device, config, control, stop, app),
        SampleFormat::I16 => build_typed_input_stream::<i16>(device, config, control, stop, app),
        SampleFormat::I24 => build_typed_input_stream::<I24>(device, config, control, stop, app),
        SampleFormat::I32 => build_typed_input_stream::<i32>(device, config, control, stop, app),
        SampleFormat::I64 => build_typed_input_stream::<i64>(device, config, control, stop, app),
        SampleFormat::U8 => build_typed_input_stream::<u8>(device, config, control, stop, app),
        SampleFormat::U16 => build_typed_input_stream::<u16>(device, config, control, stop, app),
        SampleFormat::U24 => build_typed_input_stream::<U24>(device, config, control, stop, app),
        SampleFormat::U32 => build_typed_input_stream::<u32>(device, config, control, stop, app),
        SampleFormat::U64 => build_typed_input_stream::<u64>(device, config, control, stop, app),
        SampleFormat::F32 => build_typed_input_stream::<f32>(device, config, control, stop, app),
        SampleFormat::F64 => build_typed_input_stream::<f64>(device, config, control, stop, app),
        format => Err(anyhow!("unsupported input sample format {format}")),
    }
}

fn build_typed_input_stream<T>(
    device: &Device,
    config: SupportedStreamConfig,
    control: DictationControl,
    stop: Arc<AtomicBool>,
    app: App,
) -> Result<cpal::Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let stream_config = config.clone().into();
    let mut processor = AudioProcessor::new(
        usize::from(config.channels()),
        config.sample_rate(),
        control,
        app,
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
    control: DictationControl,
    app: App,
}

impl AudioProcessor {
    fn new(channels: usize, input_sample_rate: u32, control: DictationControl, app: App) -> Self {
        Self {
            channels,
            resampler: LinearResampler::new(input_sample_rate, SAMPLE_RATE),
            spectrum_analyzer: SpectrumAnalyzer::new(SAMPLE_RATE),
            control,
            app,
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
