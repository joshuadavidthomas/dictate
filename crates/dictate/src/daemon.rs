use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::anyhow;
use dictate_desktop as delivery;
use dictate_desktop::ClipboardRestoration;
use dictate_desktop::CompletedInsertion;
use dictate_desktop::DeliveryTarget;
use dictate_desktop::DirectTypingClipboard;
use dictate_desktop::FocusObservation;
use dictate_desktop::FocusSnapshot;
use dictate_desktop::FocusedWindow;
use dictate_desktop::PushToTalkEvent;
use dictate_desktop::PushToTalkShortcut;
use dictate_desktop::UncertainInsertion;
use dictate_signal::SPECTRUM_BANDS;
use dictate_speech::CaptureHandler;
use dictate_speech::CapturedSignalMetrics;
use dictate_speech::CapturedUtterance;
use dictate_speech::DICTATION_SAMPLE_RATE;
use dictate_speech::DictationCommand;
use dictate_speech::DictationControl;
use dictate_speech::DictationFormatter;
use dictate_speech::DictationPhase;
use dictate_speech::DictationUpdate;
use dictate_speech::FinishStopping;
use dictate_speech::MicrophoneStreamError;
use dictate_speech::ModelCatalogEntry;
use dictate_speech::ProcessedDictation;
use dictate_speech::ReadyDictation;
use dictate_speech::Recognizer;
use dictate_speech::RecordSamplesUpdate;
use dictate_speech::RecordingId;
use dictate_speech::SpectrumUpdate;
use dictate_speech::TranscriptionFailure;
use dictate_speech::TranscriptionPlan;
use dictate_speech::TranscriptionResult;
use dictate_speech::capture;
use dictate_speech::save_wav_utterance;
use dictate_speech::transcribe;
use dictate_ui::Overlay;
use dictate_ui::OverlayState;
use dictate_ui::UiIdentity;

use crate::settings;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_BACKOFF_BASE: Duration = Duration::from_millis(50);
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(5);
const RESULT_NOTICE_DURATION: Duration = Duration::from_millis(1_500);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "source", content = "target", rename_all = "snake_case")]
enum RecordingDelivery {
    #[default]
    Configured,
    Override(DeliveryTarget),
}

impl RecordingDelivery {
    fn target_or(self, configured: DeliveryTarget) -> DeliveryTarget {
        match self {
            Self::Configured => configured,
            Self::Override(target) => target,
        }
    }
}

impl From<Option<DeliveryTarget>> for RecordingDelivery {
    fn from(target: Option<DeliveryTarget>) -> Self {
        match target {
            Some(target) => Self::Override(target),
            None => Self::Configured,
        }
    }
}

#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "request", rename_all = "snake_case")]
enum DaemonRequest {
    Record {
        command: DictationCommand,
        stop_focus: FocusSnapshot,
        delivery: RecordingDelivery,
    },
    PasteLast,
    Dismiss,
}

fn socket_path() -> Result<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;
    Ok(runtime_dir.join(env!("DICTATE_SOCKET_FILE")))
}

pub fn send(command: DictationCommand, delivery: Option<DeliveryTarget>) -> Result<()> {
    let stop_focus = match (command, delivery) {
        (
            DictationCommand::Stop | DictationCommand::Toggle,
            None | Some(DeliveryTarget::Insert),
        ) => dictate_desktop::snapshot(),
        (
            DictationCommand::Start | DictationCommand::Cancel,
            None
            | Some(DeliveryTarget::Stdout | DeliveryTarget::Clipboard | DeliveryTarget::Insert),
        )
        | (
            DictationCommand::Stop | DictationCommand::Toggle,
            Some(DeliveryTarget::Stdout | DeliveryTarget::Clipboard),
        ) => FocusSnapshot::Unavailable,
    };
    send_request(&DaemonRequest::Record {
        command,
        stop_focus,
        delivery: delivery.into(),
    })
}

pub fn paste_last() -> Result<()> {
    send_request(&DaemonRequest::PasteLast)
}

pub fn dismiss() -> Result<()> {
    send_request(&DaemonRequest::Dismiss)
}

fn send_request(request: &DaemonRequest) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path()?).map_err(|error| {
        anyhow!(
            "failed to connect to running {} daemon: {error}",
            env!("DICTATE_DISPLAY_NAME")
        )
    })?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    Ok(())
}

pub fn run(identity: UiIdentity, delivery_override: Option<DeliveryTarget>) -> Result<()> {
    let settings = settings::load()?;
    let plan = settings.transcription_plan(None)?;
    let delivery = delivery_override.unwrap_or_else(|| settings.delivery());
    let input_device = settings.input_device().map(str::to_owned);
    let push_to_talk = settings
        .push_to_talk()
        .map(|trigger| PushToTalkShortcut::new(env!("DICTATE_PORTAL_APP_ID"), Some(trigger)))
        .transpose()?;

    dictate_ui::run(identity, move |overlay| {
        Daemon::start(overlay, plan, delivery, input_device)?.run_in_background(push_to_talk);
        Ok(())
    })
}

fn initialize_recognizer(model: &ModelCatalogEntry) -> Result<Recognizer> {
    let model_dir = model.ensure_downloaded()?;
    model.create_recognizer(&model_dir)
}

#[derive(Clone, Debug)]
struct StopContext {
    focus: FocusSnapshot,
    delivery: DeliveryTarget,
}

#[derive(Clone)]
struct ActiveRecordingDelivery {
    target: Arc<Mutex<DeliveryTarget>>,
}

impl ActiveRecordingDelivery {
    fn new(configured: DeliveryTarget) -> Self {
        Self {
            target: Arc::new(Mutex::new(configured)),
        }
    }

    fn started(&self, requested: RecordingDelivery, configured: DeliveryTarget) {
        *self
            .target
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = requested.target_or(configured);
    }

    fn resolve(&self, requested: RecordingDelivery) -> DeliveryTarget {
        match requested {
            RecordingDelivery::Override(target) => target,
            RecordingDelivery::Configured => *self
                .target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }
}

#[derive(Default)]
struct ActivePushToTalk {
    recording_id: Option<RecordingId>,
}

impl ActivePushToTalk {
    fn activate(&mut self, daemon: &Daemon) {
        if self.recording_id.is_some() {
            return;
        }

        self.recording_id = daemon.handle_record_request(
            DictationCommand::Start,
            FocusSnapshot::Unavailable,
            RecordingDelivery::Configured,
        );
    }

    fn take_current(&mut self, current: Option<RecordingId>) -> Option<RecordingId> {
        let recording_id = self.recording_id.take()?;
        (current == Some(recording_id)).then_some(recording_id)
    }

    fn deactivate(&mut self, daemon: &Daemon, release_focus: FocusSnapshot) {
        let Some(_recording_id) = self.take_current(daemon.dictation.recording_id()) else {
            return;
        };

        let _recording_id = daemon.handle_record_request(
            DictationCommand::Stop,
            release_focus,
            RecordingDelivery::Configured,
        );
    }

    fn disconnect(&mut self, daemon: &Daemon) {
        let Some(recording_id) = self.recording_id.take() else {
            return;
        };
        if daemon.dictation.abort_recording(recording_id) {
            daemon.overlay.hide();
            eprintln!("push-to-talk recording cancelled because its shortcut session ended");
        }
    }
}

#[derive(Debug)]
enum DaemonEvent {
    Request(DaemonRequest),
    PushToTalkActivated,
    PushToTalkDeactivated(FocusSnapshot),
    PushToTalkDisconnected,
}

fn spawn_push_to_talk_listener(
    shortcut: PushToTalkShortcut,
    delivery: DeliveryTarget,
    sender: mpsc::Sender<DaemonEvent>,
) {
    thread::spawn(move || {
        eprintln!("requesting push-to-talk through the global shortcuts portal");
        let result = dictate_desktop::listen_push_to_talk(&shortcut, |event| {
            let daemon_event = match event {
                PushToTalkEvent::Activated => DaemonEvent::PushToTalkActivated,
                PushToTalkEvent::Deactivated => {
                    let focus = if delivery == DeliveryTarget::Insert {
                        dictate_desktop::snapshot()
                    } else {
                        FocusSnapshot::Unavailable
                    };
                    DaemonEvent::PushToTalkDeactivated(focus)
                }
            };
            if sender.send(daemon_event).is_err() {
                eprintln!("push-to-talk daemon event receiver ended");
            }
        });

        if sender.send(DaemonEvent::PushToTalkDisconnected).is_err() {
            eprintln!("push-to-talk daemon event receiver ended before portal cleanup");
        }
        match result {
            Ok(()) => eprintln!("push-to-talk shortcut session ended"),
            Err(error) => {
                eprintln!("push-to-talk is unavailable: {error}");
                if let Some(source) = std::error::Error::source(&error) {
                    eprintln!("push-to-talk portal cause: {source}");
                }
            }
        }
    });
}

fn apply_record_command(
    dictation: &DictationControl<StopContext>,
    recording_delivery: &ActiveRecordingDelivery,
    configured_delivery: DeliveryTarget,
    command: DictationCommand,
    stop_focus: FocusSnapshot,
    requested_delivery: RecordingDelivery,
) -> DictationUpdate {
    let stop_context = StopContext {
        focus: stop_focus,
        delivery: recording_delivery.resolve(requested_delivery),
    };
    let update = dictation.apply(command, stop_context);
    if update == DictationUpdate::Started {
        recording_delivery.started(requested_delivery, configured_delivery);
    }
    update
}

#[derive(Clone, Default)]
struct LastTranscript {
    text: Arc<Mutex<Option<Arc<ProcessedDictation>>>>,
}

impl LastTranscript {
    fn replace(&self, text: ProcessedDictation) -> Arc<ProcessedDictation> {
        let text = Arc::new(text);
        *self
            .text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&text));
        text
    }

    fn get(&self) -> Option<Arc<ProcessedDictation>> {
        self.text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct Daemon {
    socket: Arc<DaemonSocket>,
    overlay: Overlay,
    dictation: DictationControl<StopContext>,
    recording_delivery: ActiveRecordingDelivery,
    last_transcript: LastTranscript,
    plan: TranscriptionPlan,
    delivery: DeliveryTarget,
    input_device: Option<String>,
}

impl Daemon {
    fn start(
        overlay: Overlay,
        plan: TranscriptionPlan,
        delivery: DeliveryTarget,
        input_device: Option<String>,
    ) -> Result<Self> {
        let daemon = Self {
            socket: Arc::new(DaemonSocket::bind()?),
            overlay,
            dictation: DictationControl::new(),
            recording_delivery: ActiveRecordingDelivery::new(delivery),
            last_transcript: LastTranscript::default(),
            plan,
            delivery,
            input_device,
        };
        daemon.spawn_microphone_worker();

        Ok(daemon)
    }

    fn run_in_background(self, push_to_talk: Option<PushToTalkShortcut>) {
        thread::spawn(move || {
            eprintln!(
                "{} daemon ready; send a record toggle command to start dictation",
                env!("DICTATE_DISPLAY_NAME")
            );
            let (event_sender, event_receiver) = mpsc::channel();
            spawn_daemon_socket_listener(Arc::clone(&self.socket), event_sender.clone());
            if let Some(shortcut) = push_to_talk {
                spawn_push_to_talk_listener(shortcut, self.delivery, event_sender.clone());
            }
            drop(event_sender);

            let mut push_to_talk = ActivePushToTalk::default();
            while let Ok(event) = event_receiver.recv() {
                match event {
                    DaemonEvent::Request(request) => self.handle_request(request),
                    DaemonEvent::PushToTalkActivated => push_to_talk.activate(&self),
                    DaemonEvent::PushToTalkDeactivated(focus) => {
                        push_to_talk.deactivate(&self, focus);
                    }
                    DaemonEvent::PushToTalkDisconnected => push_to_talk.disconnect(&self),
                }
            }
        });
    }

    fn handle_request(&self, request: DaemonRequest) {
        match request {
            DaemonRequest::Record {
                command,
                stop_focus,
                delivery,
            } => {
                let _recording_id = self.handle_record_request(command, stop_focus, delivery);
            }
            DaemonRequest::PasteLast => self.paste_last(),
            DaemonRequest::Dismiss => self.overlay.hide(),
        }
    }

    fn handle_record_request(
        &self,
        command: DictationCommand,
        stop_focus: FocusSnapshot,
        requested_delivery: RecordingDelivery,
    ) -> Option<RecordingId> {
        let update = apply_record_command(
            &self.dictation,
            &self.recording_delivery,
            self.delivery,
            command,
            stop_focus,
            requested_delivery,
        );
        match update {
            DictationUpdate::Started => {
                self.overlay.show(OverlayState::OpeningMicrophone);
                if self.dictation.begin_recording() {
                    eprintln!("opening microphone for dictation");
                    return self.dictation.recording_id();
                }
                self.overlay.hide();
                eprintln!("recording start was superseded before microphone open");
            }
            DictationUpdate::Stopped => {
                self.overlay.show(OverlayState::Transcribing);
                if self.dictation.begin_stopping() {
                    eprintln!("dictation stopped; draining captured audio before transcription");
                } else {
                    self.overlay.hide();
                    eprintln!("recording stop was superseded before microphone drain");
                }
            }
            DictationUpdate::Cancelled => {
                self.overlay.hide();
                eprintln!("dictation cancelled");
            }
            DictationUpdate::Ignored(reason) => {
                eprintln!("record command ignored: {reason}");
            }
            DictationUpdate::Busy(DictationPhase::Unavailable) => {
                eprintln!("transcription is unavailable; restart the daemon");
            }
            DictationUpdate::Busy(phase) => {
                eprintln!("cannot change recording while {}", phase.label());
            }
        }
        None
    }

    fn paste_last(&self) {
        let phase = self.dictation.phase();
        if phase != DictationPhase::Idle {
            eprintln!("cannot paste completed dictation while {}", phase.label());
            return;
        }

        let Some(text) = self.last_transcript.get() else {
            self.overlay
                .show_briefly(OverlayState::NothingToPaste, RESULT_NOTICE_DURATION);
            eprintln!("no completed dictation is available to paste");
            return;
        };

        let report = delivery::deliver(DeliveryTarget::Insert, text.as_str());
        let overlay_state = insertion_result_overlay_state(&report);
        report_delivery(&report, text.as_str());
        show_delivery_state(&self.overlay, overlay_state);
    }

    fn spawn_microphone_worker(&self) {
        let dictation = self.dictation.clone();
        let overlay = self.overlay.clone();
        let recording_delivery = self.recording_delivery.clone();
        let last_transcript = self.last_transcript.clone();
        let plan = self.plan.clone();
        let input_device = self.input_device.clone();

        thread::spawn(move || {
            run_microphone_worker(
                &dictation,
                &recording_delivery,
                &overlay,
                &last_transcript,
                &plan,
                input_device.as_deref(),
            );
        });
    }
}

struct DictationCaptureHandler {
    dictation: DictationControl<StopContext>,
    recording_id: RecordingId,
    delivery: DeliveryTarget,
    overlay: Overlay,
}

impl CaptureHandler for DictationCaptureHandler {
    fn samples(&self, samples: &[f32]) -> SpectrumUpdate {
        match self.dictation.record_samples(self.recording_id, samples) {
            RecordSamplesUpdate::Recording => SpectrumUpdate::Emit,
            RecordSamplesUpdate::AutoStopped { duration } => {
                let focus = if self.delivery == DeliveryTarget::Insert {
                    dictate_desktop::snapshot()
                } else {
                    FocusSnapshot::Unavailable
                };
                let stop_context = StopContext {
                    focus,
                    delivery: self.delivery,
                };
                if self
                    .dictation
                    .attach_pending_stop_metadata(self.recording_id, stop_context)
                {
                    self.overlay.show(OverlayState::Transcribing);
                    if !self.dictation.begin_pending_transcription() {
                        eprintln!("auto-stop transcription handoff was superseded");
                    }
                } else {
                    eprintln!("auto-stop focus snapshot was superseded before transcription");
                }
                eprintln!(
                    "dictation reached the {} s limit; transcribing captured audio",
                    duration.as_secs()
                );
                SpectrumUpdate::Skip
            }
            RecordSamplesUpdate::Ignored => SpectrumUpdate::Skip,
        }
    }

    fn spectrum(&self, bands: [f32; SPECTRUM_BANDS]) {
        self.overlay.send_spectrum(bands);
    }

    fn stream_error(&self, error: &MicrophoneStreamError) {
        eprintln!("recording error: {error}");
        if self.dictation.abort_recording(self.recording_id) {
            self.overlay.hide();
        }
    }
}

fn run_microphone_worker(
    dictation: &DictationControl<StopContext>,
    recording_delivery: &ActiveRecordingDelivery,
    overlay: &Overlay,
    last_transcript: &LastTranscript,
    plan: &TranscriptionPlan,
    input_device: Option<&str>,
) {
    let recognizer = match initialize_recognizer(plan.model()) {
        Ok(recognizer) => recognizer,
        Err(error) => {
            eprintln!("transcription failed: {error:#}");
            overlay.hide();
            dictation.mark_unavailable();
            return;
        }
    };
    let formatter = DictationFormatter;
    let mut capture_persistence = CapturePersistence::from_env();
    let mut mic = None;
    dictation.mark_ready();
    eprintln!("transcription ready; send a record start command to start dictation");

    loop {
        thread::sleep(POLL_INTERVAL);

        let current_recording_id = dictation.recording_id();
        let open_recording_id = mic.as_ref().map(|(recording_id, _)| *recording_id);
        match mic_session_action(current_recording_id, open_recording_id) {
            MicSessionAction::Open => {
                let Some(recording_id) = dictation.recording_id() else {
                    continue;
                };
                let open_started_at = Instant::now();
                let opened_mic = match capture(
                    DICTATION_SAMPLE_RATE.as_hz(),
                    input_device,
                    DictationCaptureHandler {
                        dictation: dictation.clone(),
                        recording_id,
                        delivery: recording_delivery.resolve(RecordingDelivery::Configured),
                        overlay: overlay.clone(),
                    },
                ) {
                    Ok(opened_mic) => opened_mic,
                    Err(error) => {
                        eprintln!(
                            "microphone unavailable: {error:#}; returning to idle; send a record start command to retry"
                        );
                        if dictation.abort_recording(recording_id) {
                            overlay.hide();
                        }
                        continue;
                    }
                };
                eprintln!(
                    "microphone opened in {}ms",
                    open_started_at.elapsed().as_millis()
                );
                if dictation.recording_id() == Some(recording_id)
                    && dictation.phase() == DictationPhase::Recording
                {
                    mic = Some((recording_id, opened_mic));
                    overlay.show(OverlayState::Recording);
                    eprintln!("dictation started; send a record stop command to transcribe");
                }
            }
            MicSessionAction::Close => {
                mic = None;
            }
            MicSessionAction::Keep => {}
        }

        match dictation.finish_stopping() {
            FinishStopping::NotStopping => {}
            FinishStopping::Empty => {
                overlay.show_briefly(OverlayState::NoTranscript, RESULT_NOTICE_DURATION);
            }
            FinishStopping::Ready => {
                if !dictation.begin_pending_transcription() {
                    eprintln!("manual-stop transcription handoff was superseded");
                }
            }
        }

        let Some(ready_dictation) = dictation.take_ready_dictation() else {
            continue;
        };
        mic = None;

        let metrics = CapturedSignalMetrics::measure(ready_dictation.utterance());
        eprintln!(
            "transcribing captured audio: duration={:.3}s, samples={}, rms={:.6}",
            metrics.duration().as_secs_f64(),
            metrics.sample_count(),
            metrics.rms(),
        );
        capture_persistence.save(ready_dictation.utterance());

        transcribe_ready_dictation(
            &ready_dictation,
            &recognizer,
            formatter,
            plan,
            last_transcript,
            overlay,
        );
        dictation.finish_transcription();
    }
}

struct CapturePersistence {
    directory: Option<PathBuf>,
    next_id: u64,
}

impl CapturePersistence {
    fn from_env() -> Self {
        let directory = std::env::var_os("DICTATE_CAPTURE_DIR").map(PathBuf::from);
        if let Some(directory) = directory.as_ref() {
            eprintln!(
                "capture diagnostics enabled at {}; saved audio is retained until removed",
                directory.display()
            );
            if let Err(error) = fs::create_dir_all(directory) {
                eprintln!(
                    "warning: could not create capture diagnostics directory {}: {error}",
                    directory.display()
                );
            }
        }
        Self {
            directory,
            next_id: 1,
        }
    }

    fn save(&mut self, utterance: &CapturedUtterance) {
        let Some(directory) = self.directory.as_ref() else {
            return;
        };
        let path = next_capture_path(directory, &mut self.next_id);
        match save_wav_utterance(&path, utterance) {
            Ok(()) => eprintln!("saved captured audio to {}", path.display()),
            Err(error) => eprintln!(
                "warning: could not save captured audio to {}: {error:#}",
                path.display()
            ),
        }
    }
}

fn next_capture_path(directory: &std::path::Path, next_id: &mut u64) -> PathBuf {
    while directory.join(format!("capture-{}.wav", *next_id)).exists() {
        *next_id += 1;
    }
    let path = directory.join(format!("capture-{}.wav", *next_id));
    *next_id += 1;
    path
}

fn transcribe_ready_dictation(
    ready_dictation: &ReadyDictation<StopContext>,
    recognizer: &Recognizer,
    formatter: DictationFormatter,
    plan: &TranscriptionPlan,
    last_transcript: &LastTranscript,
    overlay: &Overlay,
) {
    match transcribe(recognizer, ready_dictation.utterance()) {
        TranscriptionResult::Transcript(raw) => {
            let text = formatter.format(&raw, plan.context());
            if text.is_empty() {
                overlay.show_briefly(OverlayState::NoTranscript, RESULT_NOTICE_DURATION);
                return;
            }

            let text = last_transcript.replace(text);
            let stop_context = ready_dictation.stop_metadata();
            let requested_delivery = stop_context.delivery;
            let (delivery_target, insert_guard) =
                guard_insert_target(requested_delivery, &stop_context.focus);
            let report = delivery::deliver(delivery_target, text.as_str());
            let overlay_state = delivery_overlay_state(requested_delivery, &report);
            report_insert_guard(insert_guard.as_ref());
            report_delivery(&report, text.as_str());
            show_delivery_state(overlay, overlay_state);
        }
        TranscriptionResult::NoTranscript(reason) => {
            match reason {
                TranscriptionFailure::TooShort(metrics) => eprintln!(
                    "{}: duration={:.3}s, samples={}, rms={:.6}",
                    reason.message(),
                    metrics.duration().as_secs_f64(),
                    metrics.sample_count(),
                    metrics.rms(),
                ),
                TranscriptionFailure::Empty | TranscriptionFailure::Noise => {
                    eprintln!("{}", reason.message());
                }
            }
            overlay.show_briefly(OverlayState::NoTranscript, RESULT_NOTICE_DURATION);
        }
    }
}

#[derive(Debug)]
enum InsertFocusGuard {
    Verified {
        target: FocusedWindow,
    },
    Changed {
        intended: FocusedWindow,
        current: FocusedWindow,
    },
    Unverifiable {
        intended: FocusSnapshot,
        current: FocusObservation,
    },
}

fn guard_insert_target(
    configured_target: DeliveryTarget,
    stop_focus: &FocusSnapshot,
) -> (DeliveryTarget, Option<InsertFocusGuard>) {
    if configured_target != DeliveryTarget::Insert {
        return (configured_target, None);
    }

    classify_insert_focus(stop_focus, dictate_desktop::observe())
}

fn classify_insert_focus(
    stop_focus: &FocusSnapshot,
    current: FocusObservation,
) -> (DeliveryTarget, Option<InsertFocusGuard>) {
    match (stop_focus, &current) {
        (FocusSnapshot::Focused(intended), FocusObservation::Focused(current))
            if intended.same_target(current) =>
        {
            (
                DeliveryTarget::Insert,
                Some(InsertFocusGuard::Verified {
                    target: current.clone(),
                }),
            )
        }
        (FocusSnapshot::Focused(intended), FocusObservation::Focused(current)) => (
            DeliveryTarget::Clipboard,
            Some(InsertFocusGuard::Changed {
                intended: intended.clone(),
                current: current.clone(),
            }),
        ),
        (intended, _) => (
            DeliveryTarget::Clipboard,
            Some(InsertFocusGuard::Unverifiable {
                intended: intended.clone(),
                current,
            }),
        ),
    }
}

fn delivery_overlay_state(
    configured_target: DeliveryTarget,
    report: &delivery::DeliveryReport,
) -> Option<OverlayState> {
    if configured_target == DeliveryTarget::Insert {
        return insertion_result_overlay_state(report);
    }

    match (configured_target, report) {
        (
            DeliveryTarget::Clipboard,
            delivery::DeliveryReport::Delivered {
                target: delivery::ConfirmedDeliveryTarget::Stdout,
                preceding_failures,
            },
        ) if !preceding_failures.is_empty() => Some(OverlayState::DeliveryFailed),
        (_, delivery::DeliveryReport::NotDelivered { .. }) => Some(OverlayState::DeliveryFailed),
        (
            _,
            delivery::DeliveryReport::Noop
            | delivery::DeliveryReport::Delivered { .. }
            | delivery::DeliveryReport::InsertCompleted(_)
            | delivery::DeliveryReport::InsertUncertain(_),
        ) => None,
    }
}

fn insertion_result_overlay_state(report: &delivery::DeliveryReport) -> Option<OverlayState> {
    match report {
        delivery::DeliveryReport::Delivered {
            target: delivery::ConfirmedDeliveryTarget::Clipboard,
            ..
        } => Some(OverlayState::PendingTranscript),
        delivery::DeliveryReport::Delivered {
            target: delivery::ConfirmedDeliveryTarget::Stdout,
            ..
        }
        | delivery::DeliveryReport::NotDelivered { .. } => Some(OverlayState::DeliveryFailed),
        delivery::DeliveryReport::Noop | delivery::DeliveryReport::InsertCompleted(_) => None,
        delivery::DeliveryReport::InsertUncertain(_) => Some(OverlayState::InsertionUncertain),
    }
}

fn show_delivery_state(overlay: &Overlay, state: Option<OverlayState>) {
    match state {
        Some(state @ (OverlayState::NoTranscript | OverlayState::NothingToPaste)) => {
            overlay.show_briefly(state, RESULT_NOTICE_DURATION);
        }
        Some(state) => overlay.show(state),
        None => overlay.hide(),
    }
}

fn report_insert_guard(guard: Option<&InsertFocusGuard>) {
    match guard {
        Some(InsertFocusGuard::Verified { target }) => {
            eprintln!("insert target matches stop focus: {target}");
        }
        Some(InsertFocusGuard::Changed { intended, current }) => {
            eprintln!(
                "insert skipped because focus changed after stop from {intended} to {current}; transcript retained while clipboard fallback is attempted"
            );
        }
        Some(InsertFocusGuard::Unverifiable { intended, current }) => {
            eprintln!(
                "insert skipped because the stop target could not be verified ({intended}; current: {current}); transcript retained while clipboard fallback is attempted"
            );
        }
        None => {}
    }
}

fn report_delivery(report: &delivery::DeliveryReport, text: &str) {
    match report {
        delivery::DeliveryReport::Noop => {}
        delivery::DeliveryReport::Delivered {
            target,
            preceding_failures,
        } => match (*target, preceding_failures.as_slice()) {
            (delivery::ConfirmedDeliveryTarget::Stdout, []) => {}
            (delivery::ConfirmedDeliveryTarget::Stdout, failures) => {
                eprintln!(
                    "dictation delivered via stdout fallback after {};",
                    describe_delivery_failures(failures)
                );
            }
            (delivery::ConfirmedDeliveryTarget::Clipboard, []) => {
                eprintln!(
                    "dictation copied to clipboard ({} chars)",
                    text.chars().count()
                );
            }
            (delivery::ConfirmedDeliveryTarget::Clipboard, failures) => {
                eprintln!(
                    "dictation copied to clipboard after {} ({} chars)",
                    describe_delivery_failures(failures),
                    text.chars().count()
                );
            }
        },
        delivery::DeliveryReport::InsertCompleted(completed) => match completed {
            CompletedInsertion::ClipboardPaste {
                transcript_bytes,
                restoration,
            } => {
                eprintln!(
                    "the clipboard paste chord was sent for a {transcript_bytes}-byte transcript; focused app insertion is not confirmed; {}",
                    describe_clipboard_restoration(restoration)
                );
            }
            CompletedInsertion::DirectTyping {
                input_bytes,
                fallback_reason,
                clipboard,
            } => match describe_direct_typing_clipboard(clipboard) {
                Some(restoration) => eprintln!(
                    "clipboard paste setup stopped before the paste chord ({fallback_reason}); direct wtype fallback completed for {input_bytes} UTF-8 bytes; {restoration}"
                ),
                None => eprintln!(
                    "clipboard paste setup stopped before the paste chord ({fallback_reason}); direct wtype fallback completed for {input_bytes} UTF-8 bytes"
                ),
            },
        },
        delivery::DeliveryReport::InsertUncertain(uncertain) => match uncertain {
            UncertainInsertion::ClipboardPaste {
                transcript_bytes,
                failure,
                restoration,
            } => {
                eprintln!(
                    "clipboard paste chord delivery is uncertain for a {transcript_bytes}-byte transcript ({failure}); direct typing was skipped to prevent duplicate insertion; {}",
                    describe_clipboard_restoration(restoration)
                );
            }
            UncertainInsertion::DirectTyping {
                maybe_input_bytes,
                fallback_reason,
                failure,
                clipboard,
            } => match describe_direct_typing_clipboard(clipboard) {
                Some(restoration) => eprintln!(
                    "clipboard paste setup stopped before the paste chord ({fallback_reason}); direct wtype fallback is uncertain after {maybe_input_bytes} UTF-8 bytes ({failure}); no further delivery was attempted; {restoration}"
                ),
                None => eprintln!(
                    "clipboard paste setup stopped before the paste chord ({fallback_reason}); direct wtype fallback is uncertain after {maybe_input_bytes} UTF-8 bytes ({failure}); no further delivery was attempted"
                ),
            },
        },
        delivery::DeliveryReport::NotDelivered { failures } => {
            eprintln!(
                "dictation was transcribed but could not be delivered: {}",
                describe_delivery_failures(failures.iter())
            );
        }
    }
}

fn describe_direct_typing_clipboard(clipboard: &DirectTypingClipboard) -> Option<String> {
    match clipboard {
        DirectTypingClipboard::NotPublished => None,
        DirectTypingClipboard::Published {
            restoration: ClipboardRestoration::Restored,
        } => Some("the prior clipboard was restored before direct typing".to_owned()),
        DirectTypingClipboard::Published {
            restoration: ClipboardRestoration::SkippedNewerClipboard,
        } => Some(
            "direct typing occurred, but prior clipboard restoration was skipped because clipboard ownership changed"
                .to_owned(),
        ),
        DirectTypingClipboard::Published {
            restoration: ClipboardRestoration::Failed(failure),
        } => Some(format!(
            "direct typing occurred, but prior clipboard restoration failed: {failure}"
        )),
    }
}

fn describe_clipboard_restoration(restoration: &ClipboardRestoration) -> String {
    match restoration {
        ClipboardRestoration::Restored => "the prior clipboard was restored".to_owned(),
        ClipboardRestoration::SkippedNewerClipboard => {
            "the ownership check found changed clipboard content, so restoration was skipped"
                .to_owned()
        }
        ClipboardRestoration::Failed(failure) => {
            format!("the prior clipboard could not be restored: {failure}")
        }
    }
}

fn describe_delivery_failures<'a>(
    failures: impl IntoIterator<Item = &'a delivery::DeliveryAttemptFailure>,
) -> String {
    failures
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MicSessionAction {
    Open,
    Close,
    Keep,
}

fn mic_session_action(
    current_recording_id: Option<RecordingId>,
    open_recording_id: Option<RecordingId>,
) -> MicSessionAction {
    match (current_recording_id, open_recording_id) {
        (Some(current), Some(open)) if current == open => MicSessionAction::Keep,
        (Some(_), None) => MicSessionAction::Open,
        (None, None) => MicSessionAction::Keep,
        (Some(_) | None, Some(_)) => MicSessionAction::Close,
    }
}

struct Backoff {
    current: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            current: ACCEPT_BACKOFF_BASE,
        }
    }

    fn next(&mut self) -> Duration {
        let current = self.current;
        self.current = self.current.saturating_mul(2).min(ACCEPT_BACKOFF_MAX);
        current
    }

    fn reset(&mut self) {
        self.current = ACCEPT_BACKOFF_BASE;
    }
}

struct DaemonSocket {
    path: PathBuf,
    listener: UnixListener,
    read_timeout: Duration,
}

impl DaemonSocket {
    fn bind() -> Result<Self> {
        Self::bind_at(socket_path()?)
    }

    fn bind_at(path: PathBuf) -> Result<Self> {
        Self::bind_at_with_read_timeout(path, CLIENT_READ_TIMEOUT)
    }

    fn bind_at_with_read_timeout(path: PathBuf, read_timeout: Duration) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                return Err(anyhow!(
                    "{} daemon socket is already in use at {}",
                    env!("DICTATE_DISPLAY_NAME"),
                    path.display()
                ));
            }
            fs::remove_file(&path)?;
        }

        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;

        Ok(Self {
            path,
            listener,
            read_timeout,
        })
    }

    fn accept(&self) -> Result<Option<UnixStream>> {
        match self.listener.accept() {
            Ok((stream, _)) => Ok(Some(stream)),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

fn spawn_daemon_socket_listener(socket: Arc<DaemonSocket>, sender: mpsc::Sender<DaemonEvent>) {
    thread::spawn(move || {
        let mut accept_backoff = Backoff::new();
        loop {
            let stream = match socket.accept() {
                Ok(Some(stream)) => {
                    accept_backoff.reset();
                    stream
                }
                Ok(None) => {
                    accept_backoff.reset();
                    thread::sleep(POLL_INTERVAL);
                    continue;
                }
                Err(error) => {
                    eprintln!("failed to accept record connection: {error:#}");
                    thread::sleep(accept_backoff.next());
                    continue;
                }
            };

            let Some(request) = read_daemon_request(stream, socket.read_timeout) else {
                continue;
            };
            if sender.send(DaemonEvent::Request(request)).is_err() {
                return;
            }
        }
    });
}

fn read_daemon_request(mut stream: UnixStream, read_timeout: Duration) -> Option<DaemonRequest> {
    if let Err(error) = stream.set_read_timeout(Some(read_timeout)) {
        eprintln!("failed to set record command timeout: {error}");
        return None;
    }

    let mut command = String::new();
    if let Err(error) = stream.read_to_string(&mut command) {
        if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
            eprintln!("record command read timed out");
        } else {
            eprintln!("failed to read record command: {error}");
        }
        return None;
    }

    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    match serde_json::from_str(command) {
        Ok(command) => Some(command),
        Err(error) => {
            eprintln!("unknown record command: {error}");
            None
        }
    }
}

impl Drop for DaemonSocket {
    fn drop(&mut self) {
        drop(fs::remove_file(&self.path));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use dictate_speech::DictationContext;
    use dictate_speech::RawTranscript;

    use super::*;

    static SOCKET_TEST_ID: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn capture_persistence_skips_sparse_existing_capture_files() {
        let id = SOCKET_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("dictate-capture-test-{}-{id}", std::process::id()));
        fs::create_dir_all(&directory).expect("create capture test directory");
        fs::write(directory.join("capture-1.wav"), []).expect("create first capture marker");
        fs::write(directory.join("capture-3.wav"), b"keep me")
            .expect("create sparse capture marker");
        let utterance =
            CapturedUtterance::new(DICTATION_SAMPLE_RATE, vec![0.0]).expect("non-empty utterance");
        let mut persistence = CapturePersistence {
            directory: Some(directory.clone()),
            next_id: 1,
        };

        persistence.save(&utterance);
        persistence.save(&utterance);

        assert!(directory.join("capture-2.wav").exists());
        assert!(directory.join("capture-4.wav").exists());
        assert_eq!(
            fs::read(directory.join("capture-3.wav")).expect("read sparse capture marker"),
            b"keep me"
        );
        fs::remove_dir_all(directory).expect("remove capture test directory");
    }

    fn focused_window(instance: u64, window_id: u64, app_id: &str, title: &str) -> FocusedWindow {
        let snapshot: FocusSnapshot = serde_json::from_value(serde_json::json!({
            "Focused": {
                "identity": {
                    "Niri": {
                        "instance": {
                            "compositor_pid": 1,
                            "start_time_ticks": instance,
                        },
                        "window_id": window_id,
                    },
                },
                "app_id": app_id,
                "title": title,
            },
        }))
        .expect("focused window fixture should deserialize");
        match snapshot {
            FocusSnapshot::Focused(window) => window,
            FocusSnapshot::NoFocusedWindow { .. } | FocusSnapshot::Unavailable => {
                panic!("focused snapshot fixture should contain a window")
            }
        }
    }

    fn socket_test_path(name: &str) -> PathBuf {
        let id = SOCKET_TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("dictate-{name}-{}-{id}.sock", std::process::id()))
    }

    #[test]
    fn daemon_requests_round_trip_with_record_focus_or_paste_action() {
        for command in [
            DictationCommand::Start,
            DictationCommand::Stop,
            DictationCommand::Toggle,
            DictationCommand::Cancel,
        ] {
            for delivery in [
                RecordingDelivery::Configured,
                RecordingDelivery::Override(DeliveryTarget::Stdout),
                RecordingDelivery::Override(DeliveryTarget::Clipboard),
                RecordingDelivery::Override(DeliveryTarget::Insert),
            ] {
                let request = DaemonRequest::Record {
                    command,
                    stop_focus: FocusSnapshot::Focused(focused_window(
                        1,
                        7,
                        "dev.editor",
                        "README.md",
                    )),
                    delivery,
                };
                let json = serde_json::to_string(&request).expect("request should serialize");

                assert_eq!(
                    serde_json::from_str::<DaemonRequest>(&json).ok(),
                    Some(request)
                );
            }
        }

        for request in [DaemonRequest::PasteLast, DaemonRequest::Dismiss] {
            let json = serde_json::to_string(&request).expect("request should serialize");
            assert_eq!(
                serde_json::from_str::<DaemonRequest>(&json).ok(),
                Some(request)
            );
        }
    }

    fn completed_recording_delivery(
        start_delivery: RecordingDelivery,
        stop_delivery: RecordingDelivery,
    ) -> DeliveryTarget {
        let dictation = DictationControl::new();
        let active = ActiveRecordingDelivery::new(DeliveryTarget::Insert);
        dictation.mark_ready();

        assert_eq!(
            apply_record_command(
                &dictation,
                &active,
                DeliveryTarget::Insert,
                DictationCommand::Start,
                FocusSnapshot::Unavailable,
                start_delivery,
            ),
            DictationUpdate::Started
        );
        assert!(dictation.begin_recording());
        let recording_id = dictation
            .recording_id()
            .expect("started recording should expose its id");
        assert_eq!(
            dictation.record_samples(recording_id, &[0.5]),
            RecordSamplesUpdate::Recording
        );
        assert_eq!(
            apply_record_command(
                &dictation,
                &active,
                DeliveryTarget::Insert,
                DictationCommand::Stop,
                FocusSnapshot::Unavailable,
                stop_delivery,
            ),
            DictationUpdate::Stopped
        );
        assert!(dictation.begin_stopping());
        assert_eq!(dictation.finish_stopping(), FinishStopping::Ready);
        assert!(dictation.begin_pending_transcription());

        dictation
            .take_ready_dictation()
            .expect("stopped recording should become ready")
            .stop_metadata()
            .delivery
    }

    #[test]
    fn push_to_talk_release_only_matches_the_recording_started_by_its_press() {
        let dictation = DictationControl::new();
        dictation.mark_ready();
        let stop_context = StopContext {
            focus: FocusSnapshot::Unavailable,
            delivery: DeliveryTarget::Clipboard,
        };

        assert_eq!(
            dictation.apply(DictationCommand::Start, stop_context.clone()),
            DictationUpdate::Started
        );
        assert!(dictation.begin_recording());
        let first = dictation
            .recording_id()
            .expect("first recording should expose its id");
        assert!(dictation.abort_recording(first));

        assert_eq!(
            dictation.apply(DictationCommand::Start, stop_context),
            DictationUpdate::Started
        );
        assert!(dictation.begin_recording());
        let second = dictation
            .recording_id()
            .expect("second recording should expose its id");

        let mut stale_shortcut = ActivePushToTalk {
            recording_id: Some(first),
        };
        assert_eq!(stale_shortcut.take_current(Some(second)), None);
        assert_eq!(stale_shortcut.recording_id, None);
        assert_eq!(dictation.recording_id(), Some(second));

        let mut current_shortcut = ActivePushToTalk {
            recording_id: Some(second),
        };
        assert_eq!(current_shortcut.take_current(Some(second)), Some(second));
        assert_eq!(current_shortcut.recording_id, None);
    }

    #[test]
    fn recording_delivery_survives_stop_and_allows_a_stop_override() {
        let clipboard = RecordingDelivery::Override(DeliveryTarget::Clipboard);
        assert_eq!(
            completed_recording_delivery(clipboard, RecordingDelivery::Configured),
            DeliveryTarget::Clipboard
        );
        assert_eq!(
            completed_recording_delivery(
                clipboard,
                RecordingDelivery::Override(DeliveryTarget::Stdout),
            ),
            DeliveryTarget::Stdout
        );
    }

    #[test]
    fn auto_stop_capture_uses_the_delivery_chosen_at_start() {
        let active = ActiveRecordingDelivery::new(DeliveryTarget::Insert);
        active.started(
            RecordingDelivery::Override(DeliveryTarget::Clipboard),
            DeliveryTarget::Insert,
        );

        assert_eq!(
            active.resolve(RecordingDelivery::Configured),
            DeliveryTarget::Clipboard
        );
    }

    #[test]
    fn configured_start_replaces_a_cancelled_recording_override() {
        let dictation = DictationControl::new();
        let active = ActiveRecordingDelivery::new(DeliveryTarget::Insert);
        dictation.mark_ready();

        assert_eq!(
            apply_record_command(
                &dictation,
                &active,
                DeliveryTarget::Insert,
                DictationCommand::Start,
                FocusSnapshot::Unavailable,
                RecordingDelivery::Override(DeliveryTarget::Clipboard),
            ),
            DictationUpdate::Started
        );
        assert!(dictation.begin_recording());
        assert_eq!(
            apply_record_command(
                &dictation,
                &active,
                DeliveryTarget::Insert,
                DictationCommand::Cancel,
                FocusSnapshot::Unavailable,
                RecordingDelivery::Configured,
            ),
            DictationUpdate::Cancelled
        );
        assert_eq!(
            apply_record_command(
                &dictation,
                &active,
                DeliveryTarget::Insert,
                DictationCommand::Start,
                FocusSnapshot::Unavailable,
                RecordingDelivery::Configured,
            ),
            DictationUpdate::Started
        );
        assert_eq!(
            active.resolve(RecordingDelivery::Configured),
            DeliveryTarget::Insert
        );
    }

    #[test]
    fn last_transcript_replaces_the_recoverable_value_atomically() {
        let store = LastTranscript::default();
        let first = DictationFormatter.format(
            &RawTranscript::new("first dictation"),
            &DictationContext::default(),
        );
        let second = DictationFormatter.format(
            &RawTranscript::new("second dictation"),
            &DictationContext::default(),
        );

        let first = store.replace(first);
        assert!(Arc::ptr_eq(
            &store.get().expect("first transcript should be retained"),
            &first
        ));

        let second = store.replace(second);
        let retained = store.get().expect("second transcript should be retained");
        assert!(Arc::ptr_eq(&retained, &second));
        assert!(!Arc::ptr_eq(&retained, &first));
    }

    #[test]
    fn rejects_old_or_unknown_daemon_request_shapes() {
        assert!(serde_json::from_str::<DaemonRequest>("\"stop\"").is_err());
        assert!(
            serde_json::from_str::<DaemonRequest>(
                r#"{"request":"record","command":"stop","stop_focus":"Unavailable"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<DaemonRequest>(
                r#"{"request":"record","command":"bogus","stop_focus":"Unavailable","delivery":{"source":"configured"}}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<DaemonRequest>(r#"{"request":"unknown"}"#).is_err());
    }

    #[test]
    fn insert_guard_accepts_same_window_id_after_title_change() {
        let stopped = FocusSnapshot::Focused(focused_window(1, 7, "dev.editor", "old title"));
        let current = FocusObservation::Focused(focused_window(1, 7, "dev.editor", "new title"));

        let (target, guard) = classify_insert_focus(&stopped, current);

        assert_eq!(target, DeliveryTarget::Insert);
        assert!(matches!(guard, Some(InsertFocusGuard::Verified { .. })));
    }

    #[test]
    fn insert_guard_uses_clipboard_when_window_id_changes() {
        let stopped = FocusSnapshot::Focused(focused_window(1, 7, "dev.editor", "README.md"));
        let current = FocusObservation::Focused(focused_window(1, 8, "dev.terminal", "Terminal"));

        let (target, guard) = classify_insert_focus(&stopped, current);

        assert_eq!(target, DeliveryTarget::Clipboard);
        assert!(matches!(guard, Some(InsertFocusGuard::Changed { .. })));
    }

    #[test]
    fn insert_guard_uses_clipboard_when_compositor_instance_changes() {
        let stopped = FocusSnapshot::Focused(focused_window(1, 7, "dev.editor", "README.md"));
        let current = FocusObservation::Focused(focused_window(2, 7, "dev.editor", "README.md"));

        let (target, guard) = classify_insert_focus(&stopped, current);

        assert_eq!(target, DeliveryTarget::Clipboard);
        assert!(matches!(guard, Some(InsertFocusGuard::Changed { .. })));
    }

    #[test]
    fn insert_guard_uses_clipboard_when_either_snapshot_is_unavailable() {
        let current = FocusObservation::Focused(focused_window(1, 7, "dev.editor", "README.md"));
        let (target, guard) = classify_insert_focus(&FocusSnapshot::Unavailable, current);
        assert_eq!(target, DeliveryTarget::Clipboard);
        assert!(matches!(guard, Some(InsertFocusGuard::Unverifiable { .. })));

        let stopped = FocusSnapshot::Focused(focused_window(1, 7, "dev.editor", "README.md"));
        let (target, guard) = classify_insert_focus(&stopped, FocusObservation::UnsupportedSession);
        assert_eq!(target, DeliveryTarget::Clipboard);
        assert!(matches!(guard, Some(InsertFocusGuard::Unverifiable { .. })));
    }

    #[test]
    fn delivery_outcome_chooses_an_accurate_overlay_state() {
        let clipboard = delivery::DeliveryReport::Delivered {
            target: delivery::ConfirmedDeliveryTarget::Clipboard,
            preceding_failures: Vec::new(),
        };
        assert_eq!(
            delivery_overlay_state(DeliveryTarget::Insert, &clipboard),
            Some(OverlayState::PendingTranscript)
        );
        assert_eq!(
            delivery_overlay_state(DeliveryTarget::Clipboard, &clipboard),
            None
        );

        let stdout_fallback = delivery::DeliveryReport::Delivered {
            target: delivery::ConfirmedDeliveryTarget::Stdout,
            preceding_failures: Vec::new(),
        };
        assert_eq!(
            delivery_overlay_state(DeliveryTarget::Insert, &stdout_fallback),
            Some(OverlayState::DeliveryFailed)
        );

        let submitted =
            delivery::DeliveryReport::InsertCompleted(CompletedInsertion::ClipboardPaste {
                transcript_bytes: 12,
                restoration: ClipboardRestoration::Restored,
            });
        assert_eq!(
            delivery_overlay_state(DeliveryTarget::Insert, &submitted),
            None
        );
    }

    #[test]
    fn direct_typing_logs_report_published_clipboard_restoration_status() {
        assert_eq!(
            describe_direct_typing_clipboard(&DirectTypingClipboard::NotPublished),
            None
        );
        assert_eq!(
            describe_direct_typing_clipboard(&DirectTypingClipboard::Published {
                restoration: ClipboardRestoration::Restored,
            })
            .as_deref(),
            Some("the prior clipboard was restored before direct typing")
        );

        let skipped = describe_direct_typing_clipboard(&DirectTypingClipboard::Published {
            restoration: ClipboardRestoration::SkippedNewerClipboard,
        })
        .expect("published clipboard status should be logged");
        assert!(skipped.contains("direct typing occurred"));
        assert!(skipped.contains("restoration was skipped"));

        let failed = describe_direct_typing_clipboard(&DirectTypingClipboard::Published {
            restoration: ClipboardRestoration::Failed(
                dictate_desktop::ClipboardTransactionFailure::TransferTimedOut {
                    operation: dictate_desktop::ClipboardOperation::VerifyMarker,
                },
            ),
        })
        .expect("published clipboard status should be logged");
        assert!(failed.contains("direct typing occurred"));
        assert!(failed.contains("restoration failed"));
    }

    #[test]
    fn mic_session_action_tracks_recording_identity() {
        let first = RecordingId::new(1);
        let second = RecordingId::new(2);

        assert_eq!(
            mic_session_action(Some(first), None),
            MicSessionAction::Open
        );
        assert_eq!(
            mic_session_action(Some(first), Some(first)),
            MicSessionAction::Keep
        );
        assert_eq!(
            mic_session_action(Some(second), Some(first)),
            MicSessionAction::Close
        );
        assert_eq!(
            mic_session_action(None, Some(first)),
            MicSessionAction::Close
        );
        assert_eq!(mic_session_action(None, None), MicSessionAction::Keep);
    }

    #[test]
    fn backoff_doubles_to_cap() {
        let mut backoff = Backoff::new();

        assert_eq!(backoff.next(), Duration::from_millis(50));
        assert_eq!(backoff.next(), Duration::from_millis(100));
        assert_eq!(backoff.next(), Duration::from_millis(200));
        assert_eq!(backoff.next(), Duration::from_millis(400));
        assert_eq!(backoff.next(), Duration::from_millis(800));
        assert_eq!(backoff.next(), Duration::from_millis(1600));
        assert_eq!(backoff.next(), Duration::from_millis(3200));
        assert_eq!(backoff.next(), Duration::from_secs(5));
        assert_eq!(backoff.next(), Duration::from_secs(5));
    }

    #[test]
    fn backoff_reset_returns_to_base() {
        let mut backoff = Backoff::new();
        let _ = backoff.next();
        let _ = backoff.next();

        backoff.reset();

        assert_eq!(backoff.next(), Duration::from_millis(50));
    }

    #[test]
    fn slow_client_does_not_block_accept_loop() {
        let path = socket_test_path("slow-client");
        let socket =
            DaemonSocket::bind_at_with_read_timeout(path.clone(), Duration::from_millis(50))
                .expect("daemon socket should bind");
        let mut client = UnixStream::connect(path).expect("client should connect");
        client
            .write_all(b"\"sta")
            .expect("partial request should write");

        let server = socket
            .accept()
            .expect("accept should succeed")
            .expect("client should be queued");
        let started = Instant::now();
        assert_eq!(read_daemon_request(server, Duration::from_millis(50)), None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn ignores_empty_clients() {
        let path = socket_test_path("empty-client");
        let socket = DaemonSocket::bind_at(path.clone()).expect("daemon socket should bind");
        drop(UnixStream::connect(path).expect("client should connect"));

        let server = socket
            .accept()
            .expect("accept should succeed")
            .expect("empty client should be queued");
        assert_eq!(read_daemon_request(server, CLIENT_READ_TIMEOUT), None);
    }

    #[test]
    fn reclaims_stale_socket_path() {
        let path = socket_test_path("stale");
        let stale_listener = UnixListener::bind(&path).expect("stale listener should bind");
        drop(stale_listener);

        let socket =
            DaemonSocket::bind_at(path.clone()).expect("daemon socket should reclaim stale path");

        assert!(path.exists());
        drop(socket);
        assert!(!path.exists());
    }
}
