use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
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
use dictate_desktop::UncertainInsertion;
#[cfg(target_os = "linux")]
use dictate_desktop_linux as platform;
#[cfg(target_os = "macos")]
use dictate_desktop_macos as platform;
#[cfg(target_os = "windows")]
use dictate_desktop_windows as platform;
use dictate_plugins::Audio;
use dictate_plugins::HotkeyPhase;
use dictate_plugins::PluginEvent;
use dictate_signal::SPECTRUM_BANDS;
use dictate_speech::CaptureHandler;
use dictate_speech::CapturedSignalMetrics;
use dictate_speech::CapturedUtterance;
use dictate_speech::DICTATION_SAMPLE_RATE;
use dictate_speech::DictationCommand;
use dictate_speech::DictationControl;
use dictate_speech::DictationPhase;
use dictate_speech::DictationUpdate;
use dictate_speech::FinishStopping;
use dictate_speech::MicrophoneStreamError;
use dictate_speech::ProcessedDictation;
use dictate_speech::ReadyDictation;
use dictate_speech::Recognizer;
use dictate_speech::RecordSamplesUpdate;
use dictate_speech::RecordingId;
use dictate_speech::SpectrumUpdate;
use dictate_speech::TranscriptionPlan;
use dictate_speech::capture;
use dictate_speech::save_wav_utterance;
use dictate_speech::transcribe;
use dictate_ui::Overlay;
use dictate_ui::OverlayState;
use dictate_ui::UiIdentity;
use directories::ProjectDirs;
use platform::AudioDucker;
use platform::DuckGuard;
use platform::PushToTalkEvent;
use platform::PushToTalkShortcut;
#[cfg(windows)]
use uds_windows::UnixListener;
#[cfg(windows)]
use uds_windows::UnixStream;

use crate::settings;

mod captures;
mod plugins;

use plugins::DecodeRequest;
use plugins::DeliveryJob;
use plugins::OwnerMessage;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const PLUGIN_RELOAD_INTERVAL: Duration = Duration::from_secs(1);
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_BACKOFF_BASE: Duration = Duration::from_millis(50);
const ACCEPT_BACKOFF_MAX: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "source", content = "target", rename_all = "snake_case")]
enum RecordingDelivery {
    #[default]
    Configured,
    Override(DeliveryTarget),
}

impl RecordingDelivery {
    fn target(self) -> Option<DeliveryTarget> {
        match self {
            Self::Configured => None,
            Self::Override(target) => Some(target),
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
    OpenWindow {
        #[serde(default)]
        activation_token: Option<String>,
    },
    /// A named hotkey went down or up. `dictate` is the push-to-talk
    /// shortcut; every name also reaches plugins.
    Hotkey {
        name: String,
        phase: HotkeyPhase,
        focus: FocusSnapshot,
    },
    /// Answered on the same connection with the daemon's own permission
    /// status; macOS attributes a CLI's permissions to the terminal that ran it.
    #[cfg(target_os = "macos")]
    Permissions,
}

pub(crate) enum InitialWindow {
    Hidden,
    Open { activation_token: Option<String> },
}

fn socket_path() -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;
        Ok(runtime_dir.join(env!("DICTATE_SOCKET_FILE")))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let dirs = project_dirs()?;
        Ok(dirs
            .data_local_dir()
            .join("run")
            .join(env!("DICTATE_SOCKET_FILE")))
    }
}

fn audio_duck_state_path() -> Result<PathBuf> {
    let dirs = project_dirs()?;
    let state_dir = dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir());
    Ok(state_dir.join("audio-duck.json"))
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", env!("DICTATE_CONFIG_DIRECTORY")).ok_or_else(|| {
        anyhow!(
            "could not determine {} application directories",
            env!("DICTATE_DISPLAY_NAME")
        )
    })
}

pub fn send(command: DictationCommand, delivery: Option<DeliveryTarget>) -> Result<()> {
    let stop_focus = match (command, delivery) {
        (
            DictationCommand::Stop | DictationCommand::Toggle,
            None | Some(DeliveryTarget::Insert),
        ) => platform::snapshot(),
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

/// The dictation hotkey's name, for plugins and `dictate hotkey`.
const DICTATION_HOTKEY: &str = "dictate";

pub fn hotkey(name: String, phase: HotkeyPhase) -> Result<()> {
    let focus = if name == DICTATION_HOTKEY && phase == HotkeyPhase::Up {
        platform::snapshot()
    } else {
        FocusSnapshot::Unavailable
    };
    send_request(&DaemonRequest::Hotkey { name, phase, focus })
}

pub fn paste_last() -> Result<()> {
    send_request(&DaemonRequest::PasteLast)
}

pub fn dismiss() -> Result<()> {
    send_request(&DaemonRequest::Dismiss)
}

pub fn open_window(activation_token: Option<String>) -> Result<()> {
    send_request(&DaemonRequest::OpenWindow { activation_token })
}

fn send_request(request: &DaemonRequest) -> Result<()> {
    write_request(request)?;
    Ok(())
}

fn write_request(request: &DaemonRequest) -> Result<UnixStream> {
    let mut stream = UnixStream::connect(socket_path()?).map_err(|error| {
        anyhow!(
            "failed to connect to running {} daemon: {error}",
            env!("DICTATE_DISPLAY_NAME")
        )
    })?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    Ok(stream)
}

/// The running daemon's permissions, or `None` when no daemon is listening.
#[cfg(target_os = "macos")]
pub fn permission_status() -> Result<Option<platform::PermissionStatus>> {
    let Ok(mut stream) = write_request(&DaemonRequest::Permissions) else {
        return Ok(None);
    };
    // The daemon reads to end of file before it answers.
    stream.shutdown(std::net::Shutdown::Write)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    if reply.trim().is_empty() {
        return Err(anyhow!(
            "the running {} daemon did not report its permissions; restart it with this build, or see its log",
            env!("DICTATE_DISPLAY_NAME")
        ));
    }
    Ok(Some(serde_json::from_str(&reply)?))
}

pub fn run(
    identity: UiIdentity,
    delivery_override: Option<DeliveryTarget>,
    initial_window: InitialWindow,
) -> Result<()> {
    let settings = settings::load()?;
    let plan = settings.transcription_plan(None)?;
    let main_window_settings = settings.main_window_settings();
    let delivery = delivery_override.unwrap_or_else(|| settings.delivery());
    let input_device = settings.input_device().map(str::to_owned);
    let duck_audio = settings.duck_audio();
    let audio_ducker = AudioDucker::new(audio_duck_state_path()?);
    let push_to_talk = settings
        .push_to_talk()
        .map(|trigger| PushToTalkShortcut::new(env!("DICTATE_PORTAL_APP_ID"), Some(trigger)))
        .transpose()?;
    let plugin_inbox = plugins::PluginInbox::new(settings, restart_args(delivery_override))?;
    let plugin_bridge = plugin_inbox.bridge();

    dictate_ui::run(
        identity,
        main_window_settings,
        plugin_bridge,
        move |overlay| {
            let window_control = overlay.clone();
            Daemon::start(
                overlay,
                plan,
                plugin_inbox,
                delivery,
                input_device,
                audio_ducker,
                duck_audio,
            )?
            .run_in_background(push_to_talk);
            if let InitialWindow::Open { activation_token } = initial_window {
                window_control.open_main_window(activation_token);
            }
            Ok(())
        },
    )
}

/// How a restart starts this daemon again: with the same delivery
/// override, and the window open, since the restart came from it.
fn restart_args(delivery_override: Option<DeliveryTarget>) -> Vec<&'static str> {
    let mut args = vec!["daemon", "--open-window"];
    if let Some(delivery) = delivery_override {
        args.push("--delivery");
        args.push(match delivery {
            DeliveryTarget::Stdout => "stdout",
            DeliveryTarget::Clipboard => "clipboard",
            DeliveryTarget::Insert => "insert",
        });
    }
    args
}

/// Replace this process with a new daemon started with `args`, so settings
/// read once at startup apply. The process keeps its id, log, and
/// permissions. Returns only when the new daemon could not be started.
#[cfg(unix)]
pub(crate) fn restart(args: &[&str]) -> anyhow::Error {
    use std::os::unix::process::CommandExt as _;

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return error.into(),
    };
    // The listener closes with this image; free its path for the new one.
    if let Ok(path) = socket_path() {
        drop(fs::remove_file(path));
    }
    eprintln!("restarting to apply the saved settings");
    std::process::Command::new(exe).args(args).exec().into()
}

#[cfg(not(unix))]
pub(crate) fn restart(_args: &[&str]) -> anyhow::Error {
    anyhow!("restarting from the window is not supported on this platform yet; restart the daemon")
}

fn initialize_recognizer(plan: &TranscriptionPlan) -> Result<Recognizer> {
    let model = plan.model();
    let model_dir = model.ensure_downloaded()?;
    let mut recognizer = model.create_recognizer(&model_dir, plan.backend(), plan.threads())?;
    let warm_up_started = Instant::now();
    recognizer.warm_up();
    eprintln!(
        "loaded {} on {} with {} decode threads; warmed up in {} ms",
        model.id().as_str(),
        recognizer.device(),
        recognizer.threads(),
        warm_up_started.elapsed().as_millis()
    );
    Ok(recognizer)
}

#[derive(Clone, Debug)]
struct StopContext {
    focus: FocusSnapshot,
}

/// Whether the push-to-talk shortcut is held, so key repeats and a lost
/// shortcut session are handled once.
#[derive(Default)]
struct ActivePushToTalk {
    held: bool,
}

impl ActivePushToTalk {
    /// Cancel the recording a held shortcut was making when its shortcut
    /// session ends.
    fn disconnect(&mut self, daemon: &Daemon) {
        if !std::mem::take(&mut self.held) {
            return;
        }
        if let Some(recording_id) = daemon.dictation.recording_id()
            && daemon.dictation.abort_recording(recording_id)
        {
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
    /// A plugin starts, stops, or cancels a recording; `done` gets the
    /// current recording afterwards. `focus` is the stop focus; observed
    /// when unavailable.
    PluginRecord {
        command: DictationCommand,
        focus: FocusSnapshot,
        done: mpsc::Sender<Option<RecordingId>>,
    },
}

/// Whether this platform can listen for `trigger` as a push-to-talk shortcut.
pub(crate) fn check_shortcut(trigger: &str) -> Result<()> {
    PushToTalkShortcut::new(env!("DICTATE_PORTAL_APP_ID"), Some(trigger))?;
    Ok(())
}

fn spawn_push_to_talk_listener(
    shortcut: PushToTalkShortcut,
    delivery: DeliveryTarget,
    sender: mpsc::Sender<DaemonEvent>,
) {
    thread::spawn(move || {
        eprintln!("requesting the configured global push-to-talk shortcut");
        let result = platform::listen_push_to_talk(&shortcut, |event| {
            let daemon_event = match event {
                PushToTalkEvent::Activated => DaemonEvent::PushToTalkActivated,
                PushToTalkEvent::Deactivated => {
                    let focus = if delivery == DeliveryTarget::Insert {
                        platform::snapshot()
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
    events: mpsc::Sender<DaemonEvent>,
    event_receiver: mpsc::Receiver<DaemonEvent>,
    plugins: mpsc::Sender<OwnerMessage>,
    overlay: Overlay,
    dictation: DictationControl<StopContext>,
    last_transcript: LastTranscript,
    delivery: DeliveryTarget,
    input_device: Option<String>,
    audio_ducker: AudioDucker,
    duck_audio: f64,
}

impl Daemon {
    fn start(
        overlay: Overlay,
        plan: TranscriptionPlan,
        plugin_inbox: plugins::PluginInbox,
        delivery: DeliveryTarget,
        input_device: Option<String>,
        audio_ducker: AudioDucker,
        duck_audio: f64,
    ) -> Result<Self> {
        let socket = Arc::new(DaemonSocket::bind()?);
        if let Err(error) = audio_ducker.recover() {
            eprintln!("system audio crash recovery is unavailable: {error}");
        }
        let dictation = DictationControl::new();
        let last_transcript = LastTranscript::default();
        let (events, event_receiver) = mpsc::channel();
        let (recognizer, decode_requests) = mpsc::channel();
        // Load the model now only when dictation uses it, so the first
        // dictation does not wait for it.
        let preload = plugin_inbox.setup.recognizer == "local";
        let plugins = plugins::spawn(
            plugins::Owner {
                dictation: dictation.clone(),
                overlay: overlay.clone(),
                last_transcript: last_transcript.clone(),
                delivery,
                events: events.clone(),
                recognizer,
                input_device: input_device.clone(),
            },
            plugin_inbox,
        );
        let daemon = Self {
            socket,
            events,
            event_receiver,
            plugins,
            overlay,
            dictation,
            last_transcript,
            delivery,
            input_device,
            audio_ducker,
            duck_audio,
        };
        spawn_recognizer(plan, preload, decode_requests, daemon.plugins.clone());
        daemon.spawn_microphone_worker();

        Ok(daemon)
    }

    fn run_in_background(self, push_to_talk: Option<PushToTalkShortcut>) {
        thread::spawn(move || {
            eprintln!(
                "{} daemon ready; send a record toggle command to start dictation",
                env!("DICTATE_DISPLAY_NAME")
            );
            spawn_daemon_socket_listener(Arc::clone(&self.socket), self.events.clone());
            if let Some(shortcut) = push_to_talk {
                spawn_push_to_talk_listener(shortcut, self.delivery, self.events.clone());
            }

            let mut push_to_talk = ActivePushToTalk::default();
            while let Ok(event) = self.event_receiver.recv() {
                match event {
                    DaemonEvent::Request(DaemonRequest::Hotkey { name, phase, focus }) => {
                        self.hotkey(&mut push_to_talk, name, phase, focus);
                    }
                    DaemonEvent::Request(request) => self.handle_request(request),
                    DaemonEvent::PushToTalkActivated => self.hotkey(
                        &mut push_to_talk,
                        DICTATION_HOTKEY.to_owned(),
                        HotkeyPhase::Down,
                        FocusSnapshot::Unavailable,
                    ),
                    DaemonEvent::PushToTalkDeactivated(focus) => self.hotkey(
                        &mut push_to_talk,
                        DICTATION_HOTKEY.to_owned(),
                        HotkeyPhase::Up,
                        focus,
                    ),
                    DaemonEvent::PushToTalkDisconnected => push_to_talk.disconnect(&self),
                    DaemonEvent::PluginRecord {
                        command,
                        focus,
                        done,
                    } => {
                        let started = self.handle_record_request(command, focus);
                        let current = started.or_else(|| self.dictation.recording_id());
                        if done.send(current).is_err() {
                            eprintln!("a plugin stopped waiting for its recording command");
                        }
                    }
                }
            }
        });
    }

    /// Plugins decide what a hotkey does. Repeated presses of the held
    /// push-to-talk shortcut are dropped.
    fn hotkey(
        &self,
        push_to_talk: &mut ActivePushToTalk,
        name: String,
        phase: HotkeyPhase,
        focus: FocusSnapshot,
    ) {
        if name == DICTATION_HOTKEY {
            let down = phase == HotkeyPhase::Down;
            if push_to_talk.held == down {
                return;
            }
            push_to_talk.held = down;
        }
        let focus =
            (phase == HotkeyPhase::Up && focus != FocusSnapshot::Unavailable).then_some(focus);
        self.send_to_plugins(PluginEvent::Hotkey { name, phase, focus });
    }

    fn send_to_plugins(&self, event: PluginEvent) {
        if self.plugins.send(OwnerMessage::Event(event)).is_err() {
            eprintln!("the plugin thread ended; the event is not delivered");
        }
    }

    fn handle_request(&self, request: DaemonRequest) {
        match request {
            DaemonRequest::Record {
                command,
                stop_focus,
                delivery,
            } => self.send_to_plugins(PluginEvent::Record {
                command,
                target: delivery.target(),
                focus: (stop_focus != FocusSnapshot::Unavailable).then_some(stop_focus),
            }),
            DaemonRequest::PasteLast => self.paste_last(),
            DaemonRequest::Dismiss => self.overlay.hide(),
            DaemonRequest::OpenWindow { activation_token } => {
                self.overlay.open_main_window(activation_token);
            }
            DaemonRequest::Hotkey { .. } => {
                eprintln!("hotkey requests are handled by the event loop");
            }
            #[cfg(target_os = "macos")]
            DaemonRequest::Permissions => {
                eprintln!("permission requests are answered by the socket listener");
            }
        }
    }

    fn handle_record_request(
        &self,
        command: DictationCommand,
        stop_focus: FocusSnapshot,
    ) -> Option<RecordingId> {
        let stop_focus = resolve_stop_focus(command, stop_focus, || {
            FocusSnapshot::from_observation(&platform::observe())
        });
        let update = self
            .dictation
            .apply(command, StopContext { focus: stop_focus });
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
            self.overlay.hide();
            eprintln!("no completed dictation is available to paste");
            return;
        };

        let report = platform::deliver(DeliveryTarget::Insert, text.as_str());
        let overlay_state = insertion_result_overlay_state(&report);
        report_delivery(&report, text.as_str());
        show_delivery_state(&self.overlay, overlay_state);
    }

    fn spawn_microphone_worker(&self) {
        let dictation = self.dictation.clone();
        let overlay = self.overlay.clone();
        let delivery = self.delivery;
        let plugins = self.plugins.clone();
        let input_device = self.input_device.clone();
        let audio_ducker = self.audio_ducker.clone();
        let duck_audio = self.duck_audio;

        thread::spawn(move || {
            let config = MicrophoneWorkerConfig {
                plugins,
                delivery,
                input_device: input_device.as_deref(),
                audio_ducker: &audio_ducker,
                duck_audio,
            };
            run_microphone_worker(&dictation, &overlay, &config);
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
                    platform::snapshot()
                } else {
                    FocusSnapshot::Unavailable
                };
                let stop_context = StopContext { focus };
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

struct MicrophoneWorkerConfig<'a> {
    /// The plugin thread, which delivers finished recordings.
    plugins: mpsc::Sender<OwnerMessage>,
    /// The configured delivery, which decides whether a stop captures focus.
    delivery: DeliveryTarget,
    input_device: Option<&'a str>,
    audio_ducker: &'a AudioDucker,
    duck_audio: f64,
}

impl MicrophoneWorkerConfig<'_> {
    fn begin_audio_duck(&self) -> Option<DuckGuard> {
        if self.duck_audio <= 0.0 {
            return None;
        }
        match self.audio_ducker.duck(self.duck_audio) {
            Ok(guard) => Some(guard),
            Err(error) => {
                eprintln!(
                    "system audio ducking is unavailable; recording continues unchanged: {error}"
                );
                None
            }
        }
    }
}

fn run_microphone_worker(
    dictation: &DictationControl<StopContext>,
    overlay: &Overlay,
    config: &MicrophoneWorkerConfig<'_>,
) {
    dictation.register_worker();
    let mut capture_persistence = CapturePersistence::from_env();
    let mut mic = None;
    let mut duck_guard = None;
    dictation.mark_ready();
    eprintln!("ready; send a record start command to start dictation");

    loop {
        // State changes unpark this thread; the timeout is only a backstop.
        thread::park_timeout(POLL_INTERVAL);

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
                    config.input_device,
                    DictationCaptureHandler {
                        dictation: dictation.clone(),
                        recording_id,
                        delivery: config.delivery,
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
                report_microphone_open(open_started_at);
                if dictation.recording_id() == Some(recording_id)
                    && dictation.phase() == DictationPhase::Recording
                {
                    mic = Some((recording_id, opened_mic));
                    if let Some(guard) = config.begin_audio_duck() {
                        drop(duck_guard.replace(guard));
                    }
                    overlay.show(OverlayState::Recording);
                    eprintln!("dictation started; send a record stop command to transcribe");
                }
            }
            MicSessionAction::Close => {
                mic = None;
                drop(duck_guard.take());
            }
            MicSessionAction::Keep => {}
        }

        match dictation.finish_stopping() {
            FinishStopping::NotStopping => {}
            FinishStopping::Empty => {
                mic = None;
                drop(duck_guard.take());
                overlay.hide();
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
        drop(duck_guard.take());

        let delivery = finalize_ready_dictation(&ready_dictation, &mut capture_persistence);
        dictation.finish_transcription();
        hand_off(&config.plugins, delivery, overlay);
    }
}

/// Serve `dictate.speech.decode` on a thread of its own, so a decode never
/// holds up the microphone or the plugins. The model loads on the first
/// decode, or at once with `preload`; a model that fails to load fails
/// every decode with the reason.
fn spawn_recognizer(
    plan: TranscriptionPlan,
    preload: bool,
    requests: mpsc::Receiver<DecodeRequest>,
    plugins: mpsc::Sender<OwnerMessage>,
) {
    thread::spawn(move || {
        let load = || {
            initialize_recognizer(&plan).map_err(|error| {
                let error = format!("the local model could not load: {error:#}");
                eprintln!("{error}");
                error
            })
        };
        let mut recognizer = preload.then(load);
        for DecodeRequest { call, audio } in requests {
            let result = match recognizer.get_or_insert_with(load) {
                Ok(recognizer) => Ok(CapturedUtterance::new(DICTATION_SAMPLE_RATE, audio.0)
                    .map_or_else(String::new, |utterance| {
                        transcribe(recognizer, &utterance).into_text()
                    })),
                Err(error) => Err(error.clone()),
            };
            if plugins
                .send(OwnerMessage::Decoded { call, result })
                .is_err()
            {
                return;
            }
        }
    });
}

/// Called after the dictation is back to idle, so the next press is not held
/// up by plugins, the paste, or the clipboard restore.
fn hand_off(plugins: &mpsc::Sender<OwnerMessage>, delivery: DeliveryJob, overlay: &Overlay) {
    if plugins.send(OwnerMessage::Job(delivery)).is_err() {
        eprintln!("the plugin thread ended; transcript not delivered");
        overlay.show(OverlayState::DeliveryFailed);
    }
}

fn report_microphone_open(started_at: Instant) {
    eprintln!(
        "microphone opened in {}ms",
        started_at.elapsed().as_millis()
    );
}

fn finalize_ready_dictation(
    ready_dictation: &ReadyDictation<StopContext>,
    capture_persistence: &mut CapturePersistence,
) -> DeliveryJob {
    let metrics = CapturedSignalMetrics::measure(ready_dictation.utterance());
    eprintln!(
        "captured audio: duration={:.3}s, samples={}, rms={:.6}",
        metrics.duration().as_secs_f64(),
        metrics.sample_count(),
        metrics.rms(),
    );
    capture_persistence.save(ready_dictation.utterance());
    DeliveryJob {
        recording: ready_dictation.recording_id(),
        audio: Audio(ready_dictation.utterance().samples().to_vec()),
        stop_context: ready_dictation.stop_metadata().clone(),
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

fn resolve_stop_focus(
    command: DictationCommand,
    stop_focus: FocusSnapshot,
    observe: impl FnOnce() -> FocusSnapshot,
) -> FocusSnapshot {
    if matches!(command, DictationCommand::Stop | DictationCommand::Toggle)
        && stop_focus == FocusSnapshot::Unavailable
    {
        observe()
    } else {
        stop_focus
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

    classify_insert_focus(stop_focus, platform::observe())
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
            CompletedInsertion::Accessibility { input_bytes } => {
                eprintln!(
                    "Accessibility inserted a {input_bytes}-byte transcript into the focused control"
                );
            }
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
                    "clipboard paste setup stopped before the paste chord ({fallback_reason}); direct input fallback completed for {input_bytes} UTF-8 bytes; {restoration}"
                ),
                None => eprintln!(
                    "clipboard paste setup stopped before the paste chord ({fallback_reason}); direct input fallback completed for {input_bytes} UTF-8 bytes"
                ),
            },
        },
        delivery::DeliveryReport::InsertUncertain(uncertain) => match uncertain {
            UncertainInsertion::Accessibility {
                maybe_input_bytes,
                failure,
            } => {
                eprintln!(
                    "Accessibility insertion is uncertain for a {maybe_input_bytes}-byte transcript ({failure}); no further delivery was attempted"
                );
            }
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
                    "clipboard paste setup stopped before the paste chord ({fallback_reason}); direct input fallback is uncertain after {maybe_input_bytes} UTF-8 bytes ({failure}); no further delivery was attempted; {restoration}"
                ),
                None => eprintln!(
                    "clipboard paste setup stopped before the paste chord ({fallback_reason}); direct input fallback is uncertain after {maybe_input_bytes} UTF-8 bytes ({failure}); no further delivery was attempted"
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

        if socket_file_exists(&path) {
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

/// Checks for the socket entry itself; Windows stores Unix sockets as reparse points that
/// `Path::exists` cannot follow.
fn socket_file_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
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

            let mut stream = stream;
            let Some(request) = read_daemon_request(&mut stream, socket.read_timeout) else {
                continue;
            };
            #[cfg(target_os = "macos")]
            if request == DaemonRequest::Permissions {
                reply_permissions(&mut stream);
                continue;
            }
            if sender.send(DaemonEvent::Request(request)).is_err() {
                return;
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn reply_permissions(stream: &mut UnixStream) {
    let status = match platform::permission_status() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("failed to read permissions for a permissions request: {error}");
            return;
        }
    };
    if let Err(error) = serde_json::to_writer(stream, &status) {
        eprintln!("failed to answer a permissions request: {error}");
    }
}

fn read_daemon_request(stream: &mut UnixStream, read_timeout: Duration) -> Option<DaemonRequest> {
    // macOS returns EINVAL after the peer closes even when its request remains buffered to EOF.
    if let Err(error) = stream.set_read_timeout(Some(read_timeout))
        && !(cfg!(target_os = "macos") && error.kind() == ErrorKind::InvalidInput)
    {
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
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use dictate_speech::DictationContext;
    use dictate_speech::DictationFormatter;
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
    fn daemon_requests_round_trip() {
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

        for request in [
            DaemonRequest::Hotkey {
                name: "meeting".to_owned(),
                phase: HotkeyPhase::Up,
                focus: FocusSnapshot::Unavailable,
            },
            DaemonRequest::PasteLast,
            DaemonRequest::Dismiss,
            DaemonRequest::OpenWindow {
                activation_token: None,
            },
            DaemonRequest::OpenWindow {
                activation_token: Some("launcher-token".to_owned()),
            },
        ] {
            let json = serde_json::to_string(&request).expect("request should serialize");
            assert_eq!(
                serde_json::from_str::<DaemonRequest>(&json).ok(),
                Some(request)
            );
        }

        assert_eq!(
            serde_json::from_str::<DaemonRequest>(r#"{"request":"open_window"}"#).ok(),
            Some(DaemonRequest::OpenWindow {
                activation_token: None,
            })
        );
    }

    #[test]
    fn unavailable_client_stop_focus_is_observed_by_the_daemon() {
        let observed = FocusSnapshot::Focused(focused_window(1, 7, "dev.editor", "README.md"));

        assert_eq!(
            resolve_stop_focus(DictationCommand::Stop, FocusSnapshot::Unavailable, || {
                observed.clone()
            },),
            observed
        );
    }

    #[test]
    fn available_client_stop_focus_is_preserved() {
        let supplied = FocusSnapshot::Focused(focused_window(1, 7, "dev.editor", "README.md"));

        assert_eq!(
            resolve_stop_focus(DictationCommand::Stop, supplied.clone(), || {
                panic!("available client focus should not be observed again")
            }),
            supplied
        );
    }

    #[test]
    fn start_does_not_capture_stop_focus() {
        assert_eq!(
            resolve_stop_focus(
                DictationCommand::Start,
                FocusSnapshot::Unavailable,
                || panic!("start should not capture stop focus"),
            ),
            FocusSnapshot::Unavailable
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

        let mut server = socket
            .accept()
            .expect("accept should succeed")
            .expect("client should be queued");
        let started = Instant::now();
        assert_eq!(
            read_daemon_request(&mut server, Duration::from_millis(50)),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn ignores_empty_clients() {
        let path = socket_test_path("empty-client");
        let socket = DaemonSocket::bind_at(path.clone()).expect("daemon socket should bind");
        drop(UnixStream::connect(path).expect("client should connect"));

        let mut server = socket
            .accept()
            .expect("accept should succeed")
            .expect("empty client should be queued");
        assert_eq!(read_daemon_request(&mut server, CLIENT_READ_TIMEOUT), None);
    }

    #[test]
    fn reads_buffered_request_after_client_disconnects() {
        let path = socket_test_path("disconnected-client");
        let socket = DaemonSocket::bind_at(path.clone()).expect("daemon socket should bind");
        let request = DaemonRequest::Record {
            command: DictationCommand::Start,
            stop_focus: FocusSnapshot::Unavailable,
            delivery: RecordingDelivery::Override(DeliveryTarget::Insert),
        };
        let mut client = UnixStream::connect(path).expect("client should connect");
        serde_json::to_writer(&mut client, &request).expect("request should serialize");
        client.write_all(b"\n").expect("request should write");
        drop(client);

        let mut server = socket
            .accept()
            .expect("accept should succeed")
            .expect("disconnected client should be queued");
        assert_eq!(
            read_daemon_request(&mut server, CLIENT_READ_TIMEOUT),
            Some(request)
        );
    }

    #[test]
    fn reclaims_stale_socket_path() {
        let path = socket_test_path("stale");
        let stale_listener = UnixListener::bind(&path).expect("stale listener should bind");
        drop(stale_listener);

        let socket =
            DaemonSocket::bind_at(path.clone()).expect("daemon socket should reclaim stale path");

        assert!(socket_file_exists(&path));
        drop(socket);
        assert!(!socket_file_exists(&path));
    }
}
