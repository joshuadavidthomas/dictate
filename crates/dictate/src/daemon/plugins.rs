//! The thread that owns the plugins.
//!
//! It runs the plugins' Lua VM, sends them hotkey and record events, and
//! serves the calls they make into the core. Dictation is the bundled
//! `dictation.lua`: a recording exists only because a plugin opened it, and
//! its audio goes only to that plugin.

use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use clap::ValueEnum as _;
use dictate_desktop::DeliveryTarget;
use dictate_desktop::FocusSnapshot;
use dictate_plugins::Audio;
use dictate_plugins::CallId;
use dictate_plugins::CoreCall;
use dictate_plugins::CoreReply;
use dictate_plugins::CoreResult;
use dictate_plugins::PluginEvent;
use dictate_plugins::PluginSetup;
use dictate_plugins::Plugins;
use dictate_plugins::SessionId;
use dictate_speech::DictationCommand;
use dictate_speech::DictationControl;
use dictate_speech::DictationPhase;
use dictate_speech::ProcessedDictation;
use dictate_speech::RecordingId;
use dictate_ui::DaemonBridge;
use dictate_ui::Overlay;
use dictate_ui::OverlayState;
use dictate_ui::SettingsEdit;
use dictate_ui::WindowRequest;

use super::DaemonEvent;
use super::LastTranscript;
use super::PLUGIN_RELOAD_INTERVAL;
use super::StopContext;
use super::captures::Captures;
use super::delivery_overlay_state;
use super::guard_insert_target;
use super::platform;
use super::report_delivery;
use super::report_insert_guard;
use super::show_delivery_state;
use crate::config_edit;
use crate::config_edit::ConfigEdit;
use crate::settings;
use crate::settings::Settings;
use crate::settings::SettingsDeliveryTarget;
use crate::settings::SettingsDictationMode;

/// How often an open recording is checked for ending without audio
/// (cancelled, or the microphone failed).
const RECORDING_POLL: Duration = Duration::from_millis(50);
/// How long a plugin's start or stop waits for the daemon's event loop.
const RECORD_ACK: Duration = Duration::from_secs(1);

/// A finished recording, from the dictation worker.
pub(super) struct DeliveryJob {
    pub(super) recording: RecordingId,
    pub(super) audio: Audio,
    pub(super) stop_context: StopContext,
}

/// Audio for the recognizer thread; its text comes back as
/// [`OwnerMessage::Decoded`].
pub(super) struct DecodeRequest {
    pub(super) call: CallId,
    pub(super) audio: Audio,
}

/// What the owner thread is woken by.
pub(super) enum OwnerMessage {
    Job(DeliveryJob),
    Event(PluginEvent),
    /// A command a plugin was waiting on finished.
    Wake,
    /// The recognizer thread decoded a [`DecodeRequest`], or could not.
    Decoded {
        call: CallId,
        result: Result<String, String>,
    },
    /// The window wants something from the plugins.
    Ui(WindowRequest),
}

/// What the owner thread needs from the daemon.
pub(super) struct Owner {
    pub(super) dictation: DictationControl<StopContext>,
    pub(super) overlay: Overlay,
    pub(super) last_transcript: LastTranscript,
    /// Where plugin deliveries go when they do not say.
    pub(super) delivery: DeliveryTarget,
    /// The daemon's event loop, which owns starting and stopping recordings.
    pub(super) events: mpsc::Sender<DaemonEvent>,
    /// The recognizer thread, which serves `dictate.speech.decode`.
    pub(super) recognizer: mpsc::Sender<DecodeRequest>,
    /// The configured microphone, for `dictate.capture.open`.
    pub(super) input_device: Option<String>,
}

/// The owner thread's inbox, made before the thread starts so the window
/// can be given a [`DaemonBridge`] to it first.
pub(super) struct PluginInbox {
    pub(super) setup: PluginSetup,
    /// The settings the daemon started with, to notice changes.
    settings: Settings,
    sender: mpsc::Sender<OwnerMessage>,
    inbox: mpsc::Receiver<OwnerMessage>,
}

impl PluginInbox {
    pub(super) fn new(settings: Settings) -> anyhow::Result<Self> {
        let (sender, inbox) = mpsc::channel();
        Ok(Self {
            setup: settings.plugin_setup()?,
            settings,
            sender,
            inbox,
        })
    }

    /// Requests from the window, answered once the owner thread runs.
    pub(super) fn bridge(&self) -> DaemonBridge {
        let sender = self.sender.clone();
        DaemonBridge::new(move |request| drop(sender.send(OwnerMessage::Ui(request))))
    }
}

/// Start the owner thread on `inbox`; send it [`OwnerMessage`]s.
pub(super) fn spawn(owner: Owner, inbox: PluginInbox) -> mpsc::Sender<OwnerMessage> {
    let PluginInbox {
        setup,
        settings,
        sender,
        inbox,
    } = inbox;
    let wake = sender.clone();
    thread::spawn(move || {
        let plugins =
            match Plugins::load(setup, Arc::new(move || drop(wake.send(OwnerMessage::Wake)))) {
                Ok(plugins) => plugins,
                Err(error) => {
                    eprintln!("plugins could not start, so dictation is unavailable: {error}");
                    return;
                }
            };
        OwnerLoop {
            owner,
            plugins,
            sessions: Sessions::default(),
            captures: Captures::default(),
            looked_over: None,
            config: ConfigWatch::new(settings),
        }
        .run(&inbox);
    });
    sender
}

fn ui_feed(feed: dictate_plugins::Feed) -> dictate_ui::Feed {
    dictate_ui::Feed {
        groups: feed
            .groups
            .into_iter()
            .map(|group| dictate_ui::FeedGroup {
                title: group.title.into(),
                rows: group
                    .rows
                    .into_iter()
                    .map(|row| dictate_ui::FeedRow {
                        text: row.text.into(),
                        detail: row.detail.map(Into::into),
                        meta: row.meta.map(Into::into),
                    })
                    .collect(),
            })
            .collect(),
        empty: feed.empty.map(Into::into),
    }
}

/// `config.toml` as last loaded, to reload it when it changes.
struct ConfigWatch {
    settings: Settings,
    /// A hash of the file as last read; `None` when it was missing or
    /// unreadable.
    seen: Option<u64>,
}

impl ConfigWatch {
    fn new(settings: Settings) -> Self {
        Self {
            seen: Self::read(),
            settings,
        }
    }

    fn read() -> Option<u64> {
        let contents = std::fs::read(settings::config_path().ok()?).ok()?;
        let mut hasher = DefaultHasher::new();
        contents.hash(&mut hasher);
        Some(hasher.finish())
    }

    /// The plugins' new setup when the file changed since last checked and
    /// loads. A file that does not load keeps the last good settings running.
    fn changed(&mut self) -> Option<PluginSetup> {
        let now = Self::read();
        if now == self.seen {
            return None;
        }
        self.seen = now;
        let loaded = settings::load().and_then(|settings| {
            let setup = settings.plugin_setup()?;
            Ok((settings, setup))
        });
        let (settings, setup) = match loaded {
            Ok(loaded) => loaded,
            Err(error) => {
                eprintln!(
                    "config.toml changed but does not load, so the last good settings stay: {error:#}"
                );
                return None;
            }
        };
        let restart = settings.needs_restart(&self.settings);
        if !restart.is_empty() {
            eprintln!(
                "config.toml changed; {} apply after the daemon restarts",
                restart.join(", ")
            );
        }
        self.settings = settings;
        Some(setup)
    }
}

/// Save one of the window's edits to `config.toml`.
fn save_edit(edit: SettingsEdit) -> anyhow::Result<()> {
    let edit = match edit {
        SettingsEdit::AddDictionary { spoken, written } => {
            ConfigEdit::AddDictionary { spoken, written }
        }
        SettingsEdit::RemoveDictionary { spoken } => ConfigEdit::RemoveDictionary { spoken },
        SettingsEdit::AddReplacement { spoken, written } => {
            ConfigEdit::AddReplacement { spoken, written }
        }
        SettingsEdit::RemoveReplacement { spoken } => ConfigEdit::RemoveReplacement { spoken },
        SettingsEdit::SetMode(key) => ConfigEdit::SetMode(
            SettingsDictationMode::from_str(&key, false).map_err(anyhow::Error::msg)?,
        ),
        SettingsEdit::SetDelivery(key) => ConfigEdit::SetDelivery(
            SettingsDeliveryTarget::from_str(&key, false).map_err(anyhow::Error::msg)?,
        ),
    };
    config_edit::apply(&settings::config_path()?, &edit)
}

struct OwnerLoop {
    config: ConfigWatch,
    owner: Owner,
    plugins: Plugins,
    sessions: Sessions<CallId>,
    captures: Captures<CallId>,
    /// An open recording that looked over, and since when.
    looked_over: Option<(RecordingId, Instant)>,
}

impl OwnerLoop {
    fn run(mut self, inbox: &mpsc::Receiver<OwnerMessage>) {
        let mut next_reload = Instant::now() + PLUGIN_RELOAD_INTERVAL;
        loop {
            let now = Instant::now();
            let mut wake = next_reload;
            if self.sessions.awaiting_recording().is_some() || self.captures.waiting() {
                wake = wake.min(now + RECORDING_POLL);
            }
            match inbox.recv_timeout(wake.saturating_duration_since(now)) {
                Ok(message) => self.handle(message),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            self.serve();
            self.answer_captures();

            let now = Instant::now();
            self.check_recording(now);
            if now >= next_reload {
                next_reload = now + PLUGIN_RELOAD_INTERVAL;
                self.reload_config();
                if self.plugins.reload_if_changed() {
                    eprintln!("plugin files changed; plugins reloaded");
                    // Unloaded plugins' sessions are cancelled.
                    self.serve();
                }
            }
        }
    }

    fn handle(&mut self, message: OwnerMessage) {
        match message {
            OwnerMessage::Job(job) => {
                let route = self.sessions.job(job);
                self.route(route);
            }
            OwnerMessage::Event(event) => self.plugins.dispatch(&event),
            OwnerMessage::Wake => self.plugins.poll(),
            OwnerMessage::Decoded { call, result } => {
                self.plugins
                    .reply(call, result.map(|text| CoreReply::Text { text }));
            }
            OwnerMessage::Ui(request) => self.answer_ui(request),
        }
    }

    /// Answer the window. A window that closed meanwhile drops its replies.
    fn answer_ui(&mut self, request: WindowRequest) {
        match request {
            WindowRequest::Pages(reply) => {
                let pages = self
                    .plugins
                    .pages()
                    .into_iter()
                    .map(|page| dictate_ui::PluginPage {
                        id: page.id.into(),
                        title: page.title.into(),
                        plugin: page.plugin.into(),
                    })
                    .collect();
                drop(reply.send(pages));
            }
            WindowRequest::Render { page, query, reply } => {
                let query = dictate_plugins::PageQuery {
                    search: query.search,
                    limit: query.limit,
                };
                let feed = self.plugins.render_page(&page, &query).map(ui_feed);
                drop(reply.send(feed));
            }
            WindowRequest::Plugins(reply) => {
                let plugins = self
                    .plugins
                    .plugins()
                    .into_iter()
                    .map(|status| dictate_ui::PluginInfo {
                        id: status.id.into(),
                        source: status.source.into(),
                        active: status.active,
                        errors: status.errors.into_iter().map(Into::into).collect(),
                    })
                    .collect();
                drop(reply.send(plugins));
            }
            WindowRequest::Settings(reply) => {
                drop(reply.send(self.config.settings.main_window_settings()));
            }
            WindowRequest::Edit { edit, reply } => {
                let saved = save_edit(edit).map_err(|error| format!("{error:#}"));
                // Apply the saved file now rather than at the next check, so
                // the reply already carries the change.
                self.reload_config();
                drop(reply.send(saved.map(|()| self.config.settings.main_window_settings())));
            }
        }
    }

    fn reload_config(&mut self) {
        if let Some(setup) = self.config.changed() {
            eprintln!("config.toml changed; settings reloaded");
            self.plugins.reconfigure(setup);
            // Plugins reloaded for new options have their sessions cancelled.
            self.serve();
        }
    }

    fn route(&mut self, route: Route<CallId>) {
        match route {
            Route::Reply(call, reply) => self.plugins.reply(call, Ok(reply)),
            Route::Ended(session) => self
                .plugins
                .dispatch(&PluginEvent::SessionEnded { session }),
            Route::Dropped => {
                if self.owner.dictation.phase() == DictationPhase::Idle {
                    self.owner.overlay.hide();
                }
            }
            Route::Unowned => {
                eprintln!("a recording ended that no plugin has open; its audio is dropped");
                if self.owner.dictation.phase() == DictationPhase::Idle {
                    self.owner.overlay.hide();
                }
            }
        }
    }

    /// Answer capture reads that have their audio.
    fn answer_captures(&mut self) {
        let answers = self.captures.answer();
        if answers.is_empty() {
            return;
        }
        for (call, answer) in answers {
            self.plugins
                .reply(call, answer.map(|audio| CoreReply::Chunk { audio }));
        }
        self.serve();
    }

    /// Serve the calls plugin code made, including calls made by code that
    /// a reply resumed.
    fn serve(&mut self) {
        loop {
            // A plugin failed with a recording in hand: that dictation is
            // lost, so say so rather than leave the overlay transcribing.
            if !self.plugins.take_failures().is_empty()
                && self.owner.dictation.phase() == DictationPhase::Idle
            {
                self.owner.overlay.show(OverlayState::DeliveryFailed);
            }
            let calls = self.plugins.take_calls();
            if calls.is_empty() {
                return;
            }
            for (call, op) in calls {
                if let Some(result) = self.call(call, op) {
                    self.plugins.reply(call, result);
                }
            }
        }
    }

    /// Serve a core call; `None` when it is answered later.
    fn call(&mut self, call: CallId, op: CoreCall) -> Option<CoreResult> {
        let dictation = &self.owner.dictation;
        let result = match op {
            CoreCall::SessionOpen => self
                .sessions
                .open(dictation.phase() == DictationPhase::Idle)
                .and_then(|()| {
                    self.record(DictationCommand::Start, FocusSnapshot::Unavailable)
                        .ok_or_else(|| "the recording could not start".to_owned())
                })
                .map(|recording| CoreReply::Session {
                    session: self.sessions.started(recording),
                }),
            CoreCall::SessionStop { session, focus } => {
                let Some(recording) = self.sessions.recording(session) else {
                    return Some(Err("no such session".to_owned()));
                };
                if dictation.recording_id() == Some(recording) {
                    self.record(
                        DictationCommand::Stop,
                        focus.unwrap_or(FocusSnapshot::Unavailable),
                    );
                }
                // Without audio yet, answered when it arrives.
                return self.sessions.stop(session, call).map(Ok);
            }
            CoreCall::Decode { audio } => {
                if self
                    .owner
                    .recognizer
                    .send(DecodeRequest { call, audio })
                    .is_ok()
                {
                    return None;
                }
                Err("the recognizer thread has stopped".to_owned())
            }
            CoreCall::SessionCancel { session } => {
                let recording = self
                    .sessions
                    .recording(session)
                    .ok_or_else(|| "no such session".to_owned());
                recording.map(|recording| {
                    self.sessions.cancel(session);
                    if self.owner.dictation.recording_id() == Some(recording) {
                        self.record(DictationCommand::Cancel, FocusSnapshot::Unavailable);
                    }
                    CoreReply::Done
                })
            }
            CoreCall::CaptureOpen { source } => self
                .captures
                .open(source, self.owner.input_device.as_deref())
                .map(|capture| CoreReply::Capture { capture }),
            CoreCall::CaptureRead { capture, samples } => {
                return self
                    .captures
                    .read(capture, call, samples)
                    .map(|answer| answer.map(|audio| CoreReply::Chunk { audio }));
            }
            CoreCall::CaptureClose { capture } => {
                if let Some((read, answer)) = self.captures.close(capture) {
                    self.plugins
                        .reply(read, answer.map(|audio| CoreReply::Chunk { audio }));
                }
                Ok(CoreReply::Done)
            }
            CoreCall::DesktopFocus => Ok(CoreReply::Focus {
                focus: FocusSnapshot::from_observation(&platform::observe()),
            }),
            CoreCall::DesktopDeliver {
                text,
                target,
                focus,
            } => self
                .deliver(
                    ProcessedDictation::new(text),
                    target.unwrap_or(self.owner.delivery),
                    &focus.unwrap_or(FocusSnapshot::Unavailable),
                )
                .map(|target| CoreReply::Delivered { target }),
            CoreCall::OverlayHide => {
                if dictation.phase() == DictationPhase::Idle {
                    self.owner.overlay.hide();
                }
                Ok(CoreReply::Done)
            }
        };
        Some(result)
    }

    /// Ask the event loop to start, stop, or cancel a recording, and wait
    /// for it to have done so. Returns the current recording afterwards.
    fn record(&self, command: DictationCommand, focus: FocusSnapshot) -> Option<RecordingId> {
        let (done, acknowledged) = mpsc::channel();
        if self
            .owner
            .events
            .send(DaemonEvent::PluginRecord {
                command,
                focus,
                done,
            })
            .is_err()
        {
            return None;
        }
        acknowledged.recv_timeout(RECORD_ACK).ok().flatten()
    }

    /// End an open recording that ended without audio.
    fn check_recording(&mut self, now: Instant) {
        let Some(recording) = self.sessions.awaiting_recording() else {
            return;
        };
        let dictation = &self.owner.dictation;
        let alive = match dictation.recording_id() {
            Some(current) => current == recording,
            // Stopped and still transcribing: only one recording runs at a
            // time, so it is this one.
            None => dictation.phase() != DictationPhase::Idle,
        };
        // The worker goes idle just before it sends the audio, so only a
        // recording that has looked over for a poll interval ended without
        // it.
        if alive {
            self.looked_over = None;
            return;
        }
        let since = match self.looked_over {
            Some((over, since)) if over == recording => since,
            _ => now,
        };
        self.looked_over = Some((recording, since));
        if now.saturating_duration_since(since) >= RECORDING_POLL
            && let Some(route) = self.sessions.ended_without_transcript()
        {
            self.route(route);
            self.serve();
        }
    }

    /// Deliver `text` with the focus guard, report it, and show the result
    /// unless a newer dictation owns the overlay. Returns where it went.
    fn deliver(
        &self,
        text: ProcessedDictation,
        requested: DeliveryTarget,
        focus: &FocusSnapshot,
    ) -> Result<DeliveryTarget, String> {
        let text = self.owner.last_transcript.replace(text);
        let (target, insert_guard) = guard_insert_target(requested, focus);
        let report = platform::deliver(target, text.as_str());
        report_insert_guard(insert_guard.as_ref());
        report_delivery(&report, text.as_str());
        if self.owner.dictation.phase() == DictationPhase::Idle {
            show_delivery_state(
                &self.owner.overlay,
                delivery_overlay_state(requested, &report),
            );
        }
        match report {
            dictate_desktop::DeliveryReport::NotDelivered { .. } => {
                Err("the text could not be delivered".to_owned())
            }
            dictate_desktop::DeliveryReport::Noop
            | dictate_desktop::DeliveryReport::Delivered { .. }
            | dictate_desktop::DeliveryReport::InsertCompleted(_)
            | dictate_desktop::DeliveryReport::InsertUncertain(_) => Ok(target),
        }
    }
}

/// The recording a plugin has open. Generic over the call id so it can be
/// tested without plugins.
struct Sessions<C> {
    next_id: u64,
    session: Option<Session<C>>,
    /// A recording a plugin cancelled; its audio is dropped.
    cancelled: Option<RecordingId>,
}

impl<C> Default for Sessions<C> {
    fn default() -> Self {
        Self {
            next_id: 0,
            session: None,
            cancelled: None,
        }
    }
}

struct Session<C> {
    id: SessionId,
    recording: RecordingId,
    /// The `Session:stop` waiting for the audio.
    stop: Option<C>,
    /// Set when the recording ended before `Session:stop`.
    ended: Option<Ended>,
}

/// How a recording ended before its plugin asked for the audio.
enum Ended {
    Recorded(DeliveryJob),
    /// Cancelled, failed, or nothing was captured.
    Silent,
}

/// Where a finished recording goes.
enum Route<C> {
    /// Answer the plugin's waiting `Session:stop`.
    Reply(C, CoreReply),
    /// Kept for `Session:stop`; tell the plugin the recording ended.
    Ended(SessionId),
    /// The plugin cancelled it.
    Dropped,
    /// No plugin has it open.
    Unowned,
}

impl<C: Copy> Sessions<C> {
    /// Whether a plugin may start a recording now.
    fn open(&mut self, idle: bool) -> Result<(), String> {
        if let Some(session) = &self.session {
            if session.ended.is_none() || session.stop.is_some() {
                return Err("a plugin already has a recording open".to_owned());
            }
            eprintln!("dropping an ended recording that its plugin never stopped");
            self.session = None;
        }
        if !idle {
            return Err("Dictate is already recording or transcribing".to_owned());
        }
        Ok(())
    }

    fn started(&mut self, recording: RecordingId) -> SessionId {
        self.next_id += 1;
        let id = SessionId(self.next_id);
        self.session = Some(Session {
            id,
            recording,
            stop: None,
            ended: None,
        });
        id
    }

    fn recording(&self, id: SessionId) -> Option<RecordingId> {
        self.session
            .as_ref()
            .filter(|session| session.id == id)
            .map(|session| session.recording)
    }

    fn job(&mut self, job: DeliveryJob) -> Route<C> {
        if Some(job.recording) == self.cancelled {
            self.cancelled = None;
            return Route::Dropped;
        }
        match self.session.as_mut() {
            Some(session) if job.recording == session.recording && session.ended.is_none() => {
                if let Some(call) = session.stop {
                    self.session = None;
                    return Route::Reply(call, recording_reply(Some(job)));
                }
                session.ended = Some(Ended::Recorded(job));
                Route::Ended(session.id)
            }
            _ => Route::Unowned,
        }
    }

    /// The plugin asks for the audio: the reply, or `None` to answer when
    /// it arrives.
    fn stop(&mut self, id: SessionId, call: C) -> Option<CoreReply> {
        let session = self.session.as_mut().filter(|session| session.id == id)?;
        if let Some(ended) = session.ended.take() {
            self.session = None;
            return Some(recording_reply(match ended {
                Ended::Recorded(job) => Some(job),
                Ended::Silent => None,
            }));
        }
        session.stop = Some(call);
        None
    }

    fn cancel(&mut self, id: SessionId) {
        if let Some(session) = self.session.take_if(|session| session.id == id)
            && session.ended.is_none()
        {
            self.cancelled = Some(session.recording);
        }
    }

    /// The recording of an open session that has no audio yet.
    fn awaiting_recording(&self) -> Option<RecordingId> {
        self.session
            .as_ref()
            .filter(|session| session.ended.is_none())
            .map(|session| session.recording)
    }

    /// The session's recording ended with nothing captured.
    fn ended_without_transcript(&mut self) -> Option<Route<C>> {
        let session = self.session.as_mut()?;
        if let Some(call) = session.stop {
            self.session = None;
            return Some(Route::Reply(call, recording_reply(None)));
        }
        session.ended = Some(Ended::Silent);
        Some(Route::Ended(session.id))
    }
}

fn recording_reply(job: Option<DeliveryJob>) -> CoreReply {
    match job {
        Some(job) => CoreReply::Recording {
            audio: job.audio,
            focus: job.stop_context.focus,
        },
        None => CoreReply::Recording {
            audio: Audio::default(),
            focus: FocusSnapshot::Unavailable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: RecordingId = RecordingId::new(1);
    const SECOND: RecordingId = RecordingId::new(2);

    fn job(recording: RecordingId, samples: usize) -> DeliveryJob {
        DeliveryJob {
            recording,
            audio: Audio(vec![0.5; samples]),
            stop_context: StopContext {
                focus: FocusSnapshot::Unavailable,
            },
        }
    }

    fn samples(reply: &CoreReply) -> usize {
        match reply {
            CoreReply::Recording { audio, .. } => audio.0.len(),
            other @ (CoreReply::Session { .. }
            | CoreReply::Text { .. }
            | CoreReply::Capture { .. }
            | CoreReply::Chunk { .. }
            | CoreReply::Focus { .. }
            | CoreReply::Delivered { .. }
            | CoreReply::Done) => panic!("expected a recording, got {other:?}"),
        }
    }

    fn opened(sessions: &mut Sessions<u32>, recording: RecordingId) -> SessionId {
        sessions.open(true).expect("should open when idle");
        sessions.started(recording)
    }

    #[test]
    fn opening_is_refused_while_busy_or_already_open() {
        let mut sessions = Sessions::<u32>::default();
        assert!(sessions.open(false).is_err());
        opened(&mut sessions, FIRST);
        assert!(sessions.open(true).is_err());
    }

    #[test]
    fn the_recording_answers_stop_whichever_comes_first() {
        // Stop first: answered when the audio arrives.
        let mut sessions = Sessions::<u32>::default();
        let id = opened(&mut sessions, FIRST);
        assert!(sessions.stop(id, 7).is_none());
        assert!(
            matches!(sessions.job(job(FIRST, 5)), Route::Reply(7, reply) if samples(&reply) == 5),
            "the waiting stop should be answered with the audio"
        );

        // Audio first (the length limit): the plugin is told, and stop
        // answers at once.
        let mut sessions = Sessions::<u32>::default();
        let id = opened(&mut sessions, FIRST);
        assert!(matches!(sessions.job(job(FIRST, 3)), Route::Ended(ended) if ended == id));
        assert_eq!(samples(&sessions.stop(id, 8).expect("should answer")), 3);
        assert!(sessions.session.is_none());
    }

    #[test]
    fn a_recording_no_plugin_has_open_goes_nowhere() {
        let mut sessions = Sessions::<u32>::default();
        assert!(matches!(sessions.job(job(FIRST, 1)), Route::Unowned));
        opened(&mut sessions, SECOND);
        assert!(matches!(sessions.job(job(FIRST, 2)), Route::Unowned));
    }

    #[test]
    fn an_ended_recording_never_stopped_gives_way_to_the_next() {
        let mut sessions = Sessions::<u32>::default();
        opened(&mut sessions, FIRST);
        assert!(matches!(sessions.job(job(FIRST, 4)), Route::Ended(_)));
        let id = opened(&mut sessions, SECOND);
        assert!(sessions.stop(id, 1).is_none(), "waits for SECOND");
    }

    #[test]
    fn cancelled_and_empty_recordings_end_the_session() {
        let mut sessions = Sessions::<u32>::default();
        let id = opened(&mut sessions, FIRST);
        sessions.cancel(id);
        assert!(matches!(sessions.job(job(FIRST, 6)), Route::Dropped));

        // Ended without audio while stop waits.
        let mut sessions = Sessions::<u32>::default();
        let id = opened(&mut sessions, FIRST);
        assert!(sessions.stop(id, 3).is_none());
        assert!(
            matches!(
                sessions.ended_without_transcript(),
                Some(Route::Reply(3, reply)) if samples(&reply) == 0
            ),
            "the waiting stop is answered with no audio"
        );
        assert!(sessions.open(true).is_ok());

        // Ended without audio before stop.
        let mut sessions = Sessions::<u32>::default();
        let id = opened(&mut sessions, FIRST);
        assert!(
            matches!(sessions.ended_without_transcript(), Some(Route::Ended(ended)) if ended == id)
        );
        assert_eq!(samples(&sessions.stop(id, 4).expect("should answer")), 0);
    }
}
