use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;

use crate::app::Overlay;
use crate::delivery;
use crate::delivery::DeliveryTarget;
use crate::dictation::DictationCommand;
use crate::dictation::DictationControl;
use crate::dictation::DictationPhase;
use crate::dictation::DictationUpdate;
use crate::dictation::FinishStopping;
use crate::focus::FocusObservation;
use crate::focus::FocusSnapshot;
use crate::focus::FocusedWindow;
use crate::models::ModelCatalogEntry;
use crate::overlay::OverlayState;
use crate::settings;
use crate::text::DictationContext;
use crate::text::DictationFormatter;
use crate::text::ProcessedDictation;
use crate::transcription::TranscriptionResult;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_BACKOFF_BASE: Duration = Duration::from_millis(50);
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(5);
const SOCKET_FILE_NAME: &str = "dictate.sock";
const RESULT_NOTICE_DURATION: Duration = Duration::from_millis(1_500);
const UNAVAILABLE_MESSAGE: &str = "transcription is unavailable; restart `dictate daemon`";

#[derive(Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "request", rename_all = "snake_case")]
enum DaemonRequest {
    Record {
        command: DictationCommand,
        stop_focus: FocusSnapshot,
    },
    PasteLast,
    Dismiss,
}

fn socket_path() -> Result<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;
    Ok(runtime_dir.join(SOCKET_FILE_NAME))
}

pub fn send(command: DictationCommand) -> Result<()> {
    let stop_focus = match command {
        DictationCommand::Stop | DictationCommand::Toggle => crate::focus::snapshot(),
        DictationCommand::Start | DictationCommand::Cancel => FocusSnapshot::Unavailable,
    };
    send_request(&DaemonRequest::Record {
        command,
        stop_focus,
    })
}

pub fn paste_last() -> Result<()> {
    send_request(&DaemonRequest::PasteLast)
}

pub fn dismiss() -> Result<()> {
    send_request(&DaemonRequest::Dismiss)
}

fn send_request(request: &DaemonRequest) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path()?)
        .map_err(|error| anyhow!("failed to connect to running Dictate daemon: {error}"))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    Ok(())
}

pub fn run(delivery_override: Option<DeliveryTarget>) -> Result<()> {
    let settings = settings::load()?;
    let model = settings.model()?;
    let context = settings.dictation_context();
    let delivery = delivery_override.unwrap_or_else(|| settings.delivery());

    crate::app::run(move |overlay| {
        Daemon::start(overlay, model, context, delivery)?.run_in_background();
        Ok(())
    })
}

fn initialize_recognizer(model: &ModelCatalogEntry) -> Result<sherpa_onnx::OfflineRecognizer> {
    let model_dir = model.ensure_downloaded()?;
    model.create_recognizer(&model_dir)
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
    socket: DaemonSocket,
    overlay: Overlay,
    dictation: DictationControl,
    last_transcript: LastTranscript,
    model: &'static ModelCatalogEntry,
    context: DictationContext,
    delivery: DeliveryTarget,
}

impl Daemon {
    fn start(
        overlay: Overlay,
        model: &'static ModelCatalogEntry,
        context: DictationContext,
        delivery: DeliveryTarget,
    ) -> Result<Self> {
        let daemon = Self {
            socket: DaemonSocket::bind()?,
            overlay,
            dictation: DictationControl::new(),
            last_transcript: LastTranscript::default(),
            model,
            context,
            delivery,
        };
        daemon.spawn_microphone_worker();

        Ok(daemon)
    }

    fn run_in_background(self) {
        thread::spawn(move || {
            eprintln!("Dictate daemon ready; run `dictate record toggle` to start dictation");
            let mut accept_backoff = Backoff::new();

            loop {
                let request = match self.socket.accept() {
                    Ok(Some(request)) => {
                        accept_backoff.reset();
                        request
                    }
                    Ok(None) => {
                        accept_backoff.reset();
                        continue;
                    }
                    Err(error) => {
                        eprintln!("failed to accept record connection: {error:#}");
                        thread::sleep(accept_backoff.next());
                        continue;
                    }
                };

                match request {
                    DaemonRequest::Record {
                        command,
                        stop_focus,
                    } => self.handle_record_request(command, stop_focus),
                    DaemonRequest::PasteLast => self.paste_last(),
                    DaemonRequest::Dismiss => self.overlay.hide(),
                }
            }
        });
    }

    fn handle_record_request(&self, command: DictationCommand, stop_focus: FocusSnapshot) {
        match self.dictation.apply(command, stop_focus) {
            DictationUpdate::Started => {
                self.overlay.show(OverlayState::Recording);
                if self.dictation.begin_recording() {
                    eprintln!("opening microphone for dictation");
                } else {
                    self.overlay.hide();
                    eprintln!("recording start was superseded before microphone open");
                }
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
                eprintln!("{UNAVAILABLE_MESSAGE}");
            }
            DictationUpdate::Busy(phase) => {
                eprintln!("cannot change recording while {}", phase.label());
            }
        }
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
        let last_transcript = self.last_transcript.clone();
        let model = self.model;
        let context = self.context.clone();
        let delivery = self.delivery;

        thread::spawn(move || {
            run_microphone_worker(
                &dictation,
                &overlay,
                &last_transcript,
                model,
                &context,
                delivery,
            );
        });
    }
}

fn run_microphone_worker(
    dictation: &DictationControl,
    overlay: &Overlay,
    last_transcript: &LastTranscript,
    model: &'static ModelCatalogEntry,
    context: &DictationContext,
    delivery: DeliveryTarget,
) {
    let recognizer = match initialize_recognizer(model) {
        Ok(recognizer) => recognizer,
        Err(error) => {
            eprintln!("transcription failed: {error:#}");
            overlay.hide();
            dictation.mark_unavailable();
            return;
        }
    };
    let formatter = DictationFormatter;
    let mut mic = None;
    dictation.mark_ready();
    eprintln!("transcription ready; run `dictate record start` to start dictation");

    loop {
        thread::sleep(POLL_INTERVAL);

        let current_recording_id = dictation.recording_id();
        let open_recording_id = mic.as_ref().map(|(recording_id, _)| *recording_id);
        match mic_session_action(current_recording_id, open_recording_id) {
            MicSessionAction::Open => {
                let Some(recording_id) = dictation.recording_id() else {
                    continue;
                };
                let opened_mic = match crate::mic::capture(
                    dictation.clone(),
                    recording_id,
                    overlay.clone(),
                ) {
                    Ok(opened_mic) => opened_mic,
                    Err(error) => {
                        eprintln!(
                            "microphone unavailable: {error:#}; returning to idle — run `dictate record start` to retry"
                        );
                        if dictation.abort_recording(recording_id) {
                            overlay.hide();
                        }
                        continue;
                    }
                };
                if dictation.recording_id() == Some(recording_id) {
                    mic = Some((recording_id, opened_mic));
                    eprintln!("dictation started; run `dictate record stop` to transcribe");
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

        match crate::transcription::transcribe(&recognizer, ready_dictation.utterance()) {
            TranscriptionResult::Transcript(raw) => {
                let text = formatter.format(&raw, context);
                if text.is_empty() {
                    overlay.show_briefly(OverlayState::NoTranscript, RESULT_NOTICE_DURATION);
                } else {
                    let text = last_transcript.replace(text);
                    let (delivery_target, insert_guard) =
                        guard_insert_target(delivery, ready_dictation.stop_focus());
                    let report = delivery::deliver(delivery_target, text.as_str());
                    let overlay_state = delivery_overlay_state(delivery, &report);
                    report_insert_guard(insert_guard.as_ref());
                    report_delivery(&report, text.as_str());
                    show_delivery_state(overlay, overlay_state);
                }
            }
            TranscriptionResult::NoTranscript(reason) => {
                eprintln!("{}", reason.message());
                overlay.show_briefly(OverlayState::NoTranscript, RESULT_NOTICE_DURATION);
            }
        }

        dictation.finish_transcription();
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

    classify_insert_focus(stop_focus, crate::focus::observe())
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
            | delivery::DeliveryReport::InsertRequestSent { .. }
            | delivery::DeliveryReport::InsertUncertain { .. },
        ) => None,
    }
}

fn insertion_result_overlay_state(report: &delivery::DeliveryReport) -> Option<OverlayState> {
    match report {
        delivery::DeliveryReport::Noop => None,
        delivery::DeliveryReport::Delivered {
            target: delivery::ConfirmedDeliveryTarget::Clipboard,
            ..
        } => Some(OverlayState::PendingTranscript),
        delivery::DeliveryReport::Delivered {
            target: delivery::ConfirmedDeliveryTarget::Stdout,
            ..
        }
        | delivery::DeliveryReport::NotDelivered { .. } => Some(OverlayState::DeliveryFailed),
        delivery::DeliveryReport::InsertRequestSent { .. } => Some(OverlayState::InsertSubmitted),
        delivery::DeliveryReport::InsertUncertain { .. } => Some(OverlayState::InsertionUncertain),
    }
}

fn show_delivery_state(overlay: &Overlay, state: Option<OverlayState>) {
    match state {
        Some(
            state @ (OverlayState::InsertSubmitted
            | OverlayState::NoTranscript
            | OverlayState::NothingToPaste),
        ) => {
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
        delivery::DeliveryReport::InsertRequestSent { sent_bytes } => {
            eprintln!(
                "dictation sent to Wayland input method ({sent_bytes} bytes); focused app insertion is not confirmed"
            );
        }
        delivery::DeliveryReport::InsertUncertain {
            maybe_sent_bytes,
            failure,
        } => {
            eprintln!(
                "insert delivery uncertain ({maybe_sent_bytes} bytes may have been sent before failure: {failure}); fallback skipped to avoid duplicating text"
            );
        }
        delivery::DeliveryReport::NotDelivered { failures } => {
            eprintln!(
                "dictation was transcribed but could not be delivered: {}",
                describe_delivery_failures(failures.iter())
            );
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
    current_recording_id: Option<crate::dictation::RecordingId>,
    open_recording_id: Option<crate::dictation::RecordingId>,
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
                    "Dictate daemon socket is already in use at {}",
                    path.display()
                ));
            }
            fs::remove_file(&path)?;
        }

        let listener = UnixListener::bind(&path)?;

        Ok(Self {
            path,
            listener,
            read_timeout,
        })
    }

    fn accept(&self) -> Result<Option<DaemonRequest>> {
        let (mut stream, _) = self.listener.accept()?;
        stream.set_read_timeout(Some(self.read_timeout))?;

        let mut command = String::new();
        if let Err(error) = stream.read_to_string(&mut command) {
            if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
                eprintln!("record command read timed out");
            } else {
                eprintln!("failed to read record command: {error}");
            }
            return Ok(None);
        }

        let command = command.trim();
        if command.is_empty() {
            return Ok(None);
        }

        match serde_json::from_str(command) {
            Ok(command) => Ok(Some(command)),
            Err(error) => {
                eprintln!("unknown record command: {error}");
                Ok(None)
            }
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

    use super::*;

    static SOCKET_TEST_ID: AtomicUsize = AtomicUsize::new(0);

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
            let request = DaemonRequest::Record {
                command,
                stop_focus: FocusSnapshot::Focused(FocusedWindow::test_niri(
                    1,
                    7,
                    "dev.editor",
                    "README.md",
                )),
            };
            let json = serde_json::to_string(&request).expect("request should serialize");

            assert_eq!(
                serde_json::from_str::<DaemonRequest>(&json).ok(),
                Some(request)
            );
        }

        for request in [DaemonRequest::PasteLast, DaemonRequest::Dismiss] {
            let json = serde_json::to_string(&request).expect("request should serialize");
            assert_eq!(
                serde_json::from_str::<DaemonRequest>(&json).ok(),
                Some(request)
            );
        }
    }

    #[test]
    fn last_transcript_replaces_the_recoverable_value_atomically() {
        let store = LastTranscript::default();
        let first = DictationFormatter.format(
            &crate::transcription::RawTranscript::new("first dictation"),
            &DictationContext::default(),
        );
        let second = DictationFormatter.format(
            &crate::transcription::RawTranscript::new("second dictation"),
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
                r#"{"request":"record","command":"bogus","stop_focus":"Unavailable"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<DaemonRequest>(r#"{"request":"unknown"}"#).is_err());
    }

    #[test]
    fn insert_guard_accepts_same_window_id_after_title_change() {
        let stopped =
            FocusSnapshot::Focused(FocusedWindow::test_niri(1, 7, "dev.editor", "old title"));
        let current =
            FocusObservation::Focused(FocusedWindow::test_niri(1, 7, "dev.editor", "new title"));

        let (target, guard) = classify_insert_focus(&stopped, current);

        assert_eq!(target, DeliveryTarget::Insert);
        assert!(matches!(guard, Some(InsertFocusGuard::Verified { .. })));
    }

    #[test]
    fn insert_guard_uses_clipboard_when_window_id_changes() {
        let stopped =
            FocusSnapshot::Focused(FocusedWindow::test_niri(1, 7, "dev.editor", "README.md"));
        let current =
            FocusObservation::Focused(FocusedWindow::test_niri(1, 8, "dev.terminal", "Terminal"));

        let (target, guard) = classify_insert_focus(&stopped, current);

        assert_eq!(target, DeliveryTarget::Clipboard);
        assert!(matches!(guard, Some(InsertFocusGuard::Changed { .. })));
    }

    #[test]
    fn insert_guard_uses_clipboard_when_compositor_instance_changes() {
        let stopped =
            FocusSnapshot::Focused(FocusedWindow::test_niri(1, 7, "dev.editor", "README.md"));
        let current =
            FocusObservation::Focused(FocusedWindow::test_niri(2, 7, "dev.editor", "README.md"));

        let (target, guard) = classify_insert_focus(&stopped, current);

        assert_eq!(target, DeliveryTarget::Clipboard);
        assert!(matches!(guard, Some(InsertFocusGuard::Changed { .. })));
    }

    #[test]
    fn insert_guard_uses_clipboard_when_either_snapshot_is_unavailable() {
        let current =
            FocusObservation::Focused(FocusedWindow::test_niri(1, 7, "dev.editor", "README.md"));
        let (target, guard) = classify_insert_focus(&FocusSnapshot::Unavailable, current);
        assert_eq!(target, DeliveryTarget::Clipboard);
        assert!(matches!(guard, Some(InsertFocusGuard::Unverifiable { .. })));

        let stopped =
            FocusSnapshot::Focused(FocusedWindow::test_niri(1, 7, "dev.editor", "README.md"));
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

        let submitted = delivery::DeliveryReport::InsertRequestSent { sent_bytes: 12 };
        assert_eq!(
            delivery_overlay_state(DeliveryTarget::Insert, &submitted),
            Some(OverlayState::InsertSubmitted)
        );
    }

    #[test]
    fn mic_session_action_tracks_recording_identity() {
        let first = crate::dictation::RecordingId::new(1);
        let second = crate::dictation::RecordingId::new(2);

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

        let started = Instant::now();
        assert_eq!(
            socket.accept().expect("accept should time out cleanly"),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn ignores_empty_clients() {
        let path = socket_test_path("empty-client");
        let socket = DaemonSocket::bind_at(path.clone()).expect("daemon socket should bind");
        drop(UnixStream::connect(path).expect("client should connect"));

        assert_eq!(
            socket.accept().expect("empty client should be ignored"),
            None
        );
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
