//! The thread that owns the plugin host.
//!
//! It runs the transcript chain on finished dictations, sends hotkey events
//! to plugins, and serves the calls plugins make into the core. It never
//! waits on the host: the host's messages arrive on the same channel as
//! everything else.
//!
//! The core moves first on every press: the dictation hotkey starts a
//! recording before plugins hear about it. A plugin that calls
//! `dictate.session.open` within [`CLAIM_WINDOW`] of the key going down
//! adopts that recording, and its transcript goes to the plugin instead of
//! the built-in delivery. Every other recording, and an adopted one whose
//! plugin host dies or does not ask for the transcript, is delivered by the
//! built-in flow, so the words spoken are not lost.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use dictate_desktop::DeliveryTarget;
use dictate_desktop::FocusSnapshot;
use dictate_plugins::CallId;
use dictate_plugins::CoreCall;
use dictate_plugins::CoreReply;
use dictate_plugins::CoreResult;
use dictate_plugins::HostEvent;
use dictate_plugins::HostId;
use dictate_plugins::HostMessage;
use dictate_plugins::PluginEvent;
use dictate_plugins::PluginProcess;
use dictate_plugins::PluginSetup;
use dictate_plugins::SessionId;
use dictate_speech::DictationCommand;
use dictate_speech::DictationControl;
use dictate_speech::DictationPhase;
use dictate_speech::ProcessedDictation;
use dictate_speech::RawTranscript;
use dictate_speech::RecordingId;
use dictate_ui::Overlay;

use super::DaemonEvent;
use super::LastTranscript;
use super::PLUGIN_RELOAD_INTERVAL;
use super::StopContext;
use super::delivery_overlay_state;
use super::guard_insert_target;
use super::platform;
use super::report_delivery;
use super::report_insert_guard;
use super::show_delivery_state;
use super::transcript_context;

/// How long after the key goes down a plugin may claim the recording.
const CLAIM_WINDOW: Duration = Duration::from_millis(50);
/// How long an adopted transcript waits for `Session:stop` before the
/// built-in flow delivers it.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// How often an adopted recording is checked for ending without a
/// transcript (cancelled, or the microphone failed).
const RECORDING_POLL: Duration = Duration::from_millis(50);
/// How long a plugin's start or stop waits for the daemon's event loop.
const RECORD_ACK: Duration = Duration::from_secs(1);

/// A finished recording, from the dictation worker.
pub(super) struct DeliveryJob {
    pub(super) recording: Option<RecordingId>,
    /// `None` when nothing was heard.
    pub(super) raw: Option<RawTranscript>,
    pub(super) stop_context: StopContext,
}

/// What the owner thread is woken by.
pub(super) enum OwnerMessage {
    Job(DeliveryJob),
    Host(HostMessage),
    /// A hotkey went down or up. `recording` is the recording a `down` of the
    /// dictation hotkey started.
    Hotkey {
        event: PluginEvent,
        recording: Option<RecordingId>,
        at: Instant,
    },
}

/// What the owner thread needs from the daemon.
pub(super) struct Owner {
    pub(super) dictation: DictationControl<StopContext>,
    pub(super) overlay: Overlay,
    pub(super) plugin_setup: PluginSetup,
    pub(super) last_transcript: LastTranscript,
    /// Where plugin deliveries go when they do not say.
    pub(super) delivery: DeliveryTarget,
    /// The daemon's event loop, which owns starting and stopping recordings.
    pub(super) events: mpsc::Sender<DaemonEvent>,
}

/// Start the owner thread; send it [`OwnerMessage`]s.
pub(super) fn spawn(owner: Owner) -> mpsc::Sender<OwnerMessage> {
    let (sender, inbox) = mpsc::channel();
    let notify = sender.clone();
    thread::spawn(move || {
        let process = crate::cli::plugin_host(
            owner.plugin_setup.clone(),
            Arc::new(move |message| drop(notify.send(OwnerMessage::Host(message)))),
        );
        OwnerLoop {
            owner,
            process,
            sessions: Sessions::default(),
            native: VecDeque::new(),
            next_job: 1,
            looked_over: None,
        }
        .run(&inbox);
    });
    sender
}

/// A recording the built-in flow delivers, in order.
struct NativeJob {
    id: u64,
    stop_context: StopContext,
    /// Set when the transcript chain finished.
    text: Option<ProcessedDictation>,
}

struct OwnerLoop {
    owner: Owner,
    process: PluginProcess,
    sessions: Sessions<HostId, CallId>,
    native: VecDeque<NativeJob>,
    next_job: u64,
    /// An adopted recording that looked over, and since when.
    looked_over: Option<(RecordingId, Instant)>,
}

impl OwnerLoop {
    fn run(mut self, inbox: &mpsc::Receiver<OwnerMessage>) {
        let mut next_reload = Instant::now() + PLUGIN_RELOAD_INTERVAL;
        loop {
            let now = Instant::now();
            let mut wake = next_reload;
            for deadline in [self.process.next_deadline(), self.sessions.next_deadline()]
                .into_iter()
                .flatten()
            {
                wake = wake.min(deadline);
            }
            if self.sessions.awaiting_recording().is_some() {
                wake = wake.min(now + RECORDING_POLL);
            }
            let events = match inbox.recv_timeout(wake.saturating_duration_since(now)) {
                Ok(message) => self.handle(message),
                Err(mpsc::RecvTimeoutError::Timeout) => Vec::new(),
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            self.serve(events);

            let now = Instant::now();
            let events = self.process.check_deadlines();
            self.serve(events);
            self.check_recording(now);
            if let Some(job) = self.sessions.expire(now) {
                eprintln!("a plugin adopted a recording but never asked for it; delivering it");
                self.start_native(job);
            }
            if now >= next_reload {
                next_reload = now + PLUGIN_RELOAD_INTERVAL;
                if self.owner.dictation.phase() == DictationPhase::Idle
                    && self.process.reload_if_changed()
                {
                    eprintln!("plugin files changed; starting a fresh plugin host");
                }
            }
        }
    }

    fn handle(&mut self, message: OwnerMessage) -> Vec<HostEvent> {
        match message {
            OwnerMessage::Job(job) => match self.sessions.job(job) {
                JobRoute::Native(job) => {
                    self.start_native(job);
                    Vec::new()
                }
                JobRoute::Reply(call, reply) => self.process.reply(call, Ok(reply)),
                JobRoute::Held | JobRoute::Dropped => Vec::new(),
            },
            OwnerMessage::Host(message) => self.process.receive(message),
            OwnerMessage::Hotkey {
                event,
                recording,
                at,
            } => {
                if let Some(recording) = recording {
                    self.sessions.pressed(recording, at);
                }
                self.process.event(event)
            }
        }
    }

    /// Act on what the plugin host reported, including the replies that
    /// acting produces.
    fn serve(&mut self, mut events: Vec<HostEvent>) {
        while !events.is_empty() {
            for event in std::mem::take(&mut events) {
                match event {
                    HostEvent::Processed { id, text } => {
                        if let Some(job) = self.native.iter_mut().find(|job| job.id == id) {
                            job.text = Some(text);
                        }
                        self.deliver_ready();
                    }
                    HostEvent::Call { call, op } => {
                        if let Some(result) = self.call(call, op) {
                            events.extend(self.process.reply(call, result));
                        }
                    }
                    HostEvent::HostStopped(host) => {
                        if let Some(job) = self.sessions.host_stopped(host) {
                            eprintln!(
                                "the plugin host stopped while it owned a recording; delivering it"
                            );
                            self.start_native(job);
                        }
                    }
                }
            }
        }
    }

    /// Serve a core call; `None` when it is answered later.
    fn call(&mut self, call: CallId, op: CoreCall) -> Option<CoreResult> {
        if let CoreCall::SessionStop { session } = op {
            let Some(recording) = self.sessions.recording(session) else {
                return Some(Err("no such session".to_owned()));
            };
            if self.owner.dictation.recording_id() == Some(recording) {
                self.record(DictationCommand::Stop);
            }
            // Without a transcript yet, answered when it arrives.
            return self.sessions.stop(session, call).map(Ok);
        }
        Some(self.call_now(call, op))
    }

    fn call_now(&mut self, call: CallId, op: CoreCall) -> CoreResult {
        let dictation = &self.owner.dictation;
        match op {
            CoreCall::SessionOpen => {
                let idle = dictation.phase() == DictationPhase::Idle;
                match self.sessions.open(
                    call.host(),
                    dictation.recording_id(),
                    idle,
                    Instant::now(),
                ) {
                    Open::Adopted(session) => Ok(CoreReply::Session { session }),
                    Open::Refused(why) => Err(why),
                    Open::Start => {
                        let recording = self
                            .record(DictationCommand::Start)
                            .ok_or_else(|| "the recording could not start".to_owned())?;
                        let session = self.sessions.started(call.host(), recording);
                        Ok(CoreReply::Session { session })
                    }
                }
            }
            CoreCall::SessionStop { .. } => Err("handled by `call`".to_owned()),
            CoreCall::SessionCancel { session } => {
                let recording = self.sessions.recording(session).ok_or("no such session")?;
                self.sessions.cancel(session);
                if dictation.recording_id() == Some(recording) {
                    self.record(DictationCommand::Cancel);
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
        }
    }

    /// Ask the event loop to start, stop, or cancel a recording, and wait
    /// for it to have done so. Returns the current recording afterwards.
    fn record(&self, command: DictationCommand) -> Option<RecordingId> {
        let (done, acknowledged) = mpsc::channel();
        if self
            .owner
            .events
            .send(DaemonEvent::PluginRecord { command, done })
            .is_err()
        {
            return None;
        }
        acknowledged.recv_timeout(RECORD_ACK).ok().flatten()
    }

    /// End an adopted recording that ended without a transcript.
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
        // The worker goes idle just before it sends the transcript, so only
        // a recording that has looked over for a poll interval ended
        // without one.
        if alive {
            self.looked_over = None;
            return;
        }
        let since = match self.looked_over {
            Some((over, since)) if over == recording => since,
            _ => now,
        };
        self.looked_over = Some((recording, since));
        let over = now.saturating_duration_since(since) >= RECORDING_POLL;
        if over && let Some((call, reply)) = self.sessions.ended_without_transcript(now) {
            let events = self.process.reply(call, Ok(reply));
            self.serve(events);
        }
    }

    fn start_native(&mut self, job: DeliveryJob) {
        let Some(raw) = job.raw else {
            return;
        };
        let id = self.next_job;
        self.next_job += 1;
        let context = transcript_context(&job.stop_context.focus);
        self.native.push_back(NativeJob {
            id,
            stop_context: job.stop_context,
            text: None,
        });
        let events = self.process.transcript(id, &raw, &context);
        self.serve(events);
    }

    /// Deliver finished native jobs in the order they were recorded.
    fn deliver_ready(&mut self) {
        while self.native.front().is_some_and(|job| job.text.is_some()) {
            let Some(NativeJob {
                stop_context,
                text: Some(text),
                ..
            }) = self.native.pop_front()
            else {
                return;
            };
            if text.is_empty() {
                if self.owner.dictation.phase() == DictationPhase::Idle {
                    self.owner.overlay.hide();
                }
                continue;
            }
            drop(self.deliver(text, stop_context.delivery, &stop_context.focus));
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

/// Which recordings belong to plugins. Generic over the host and call ids
/// so it can be tested without a plugin host.
struct Sessions<H, C> {
    next_id: u64,
    /// The latest dictation-hotkey press a plugin may still claim.
    press: Option<(RecordingId, Instant)>,
    /// The one recording a plugin owns.
    session: Option<Session<H, C>>,
    /// A recording a plugin cancelled; its transcript is dropped.
    cancelled: Option<RecordingId>,
}

impl<H, C> Default for Sessions<H, C> {
    fn default() -> Self {
        Self {
            next_id: 0,
            press: None,
            session: None,
            cancelled: None,
        }
    }
}

struct Session<H, C> {
    id: SessionId,
    host: H,
    recording: RecordingId,
    /// The `Session:stop` waiting for the transcript.
    stop: Option<C>,
    /// The transcript, once it arrived, and until when it waits for
    /// `Session:stop`. `None` inside means the recording ended without
    /// one.
    ended: Option<(Option<DeliveryJob>, Instant)>,
}

enum Open {
    Adopted(SessionId),
    /// Nothing is recording; start a recording for the plugin.
    Start,
    Refused(String),
}

enum JobRoute<C> {
    /// Nobody claimed it; the built-in flow delivers it.
    Native(DeliveryJob),
    /// Answer the plugin's waiting `Session:stop`.
    Reply(C, CoreReply),
    /// Kept until the plugin asks for it.
    Held,
    /// The plugin cancelled it.
    Dropped,
}

impl<H: Copy + PartialEq, C: Copy> Sessions<H, C> {
    fn pressed(&mut self, recording: RecordingId, at: Instant) {
        self.press = Some((recording, at));
    }

    fn open(&mut self, host: H, current: Option<RecordingId>, idle: bool, now: Instant) -> Open {
        if self.session.is_some() {
            return Open::Refused("a plugin already has a recording open".to_owned());
        }
        match (self.press, current) {
            (Some((pressed, at)), Some(current)) if pressed == current => {
                if now.saturating_duration_since(at) > CLAIM_WINDOW {
                    return Open::Refused(format!(
                        "too late: a plugin must claim the press within {} ms; Dictate delivers this one",
                        CLAIM_WINDOW.as_millis()
                    ));
                }
                self.press = None;
                Open::Adopted(self.adopt(host, current))
            }
            _ if idle => Open::Start,
            _ => Open::Refused("Dictate is already recording or transcribing".to_owned()),
        }
    }

    fn started(&mut self, host: H, recording: RecordingId) -> SessionId {
        self.adopt(host, recording)
    }

    fn adopt(&mut self, host: H, recording: RecordingId) -> SessionId {
        self.next_id += 1;
        let id = SessionId(self.next_id);
        self.session = Some(Session {
            id,
            host,
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

    fn job(&mut self, job: DeliveryJob) -> JobRoute<C> {
        if job.recording.is_some() && job.recording == self.cancelled {
            self.cancelled = None;
            return JobRoute::Dropped;
        }
        match self.session.as_mut() {
            Some(session)
                if job.recording == Some(session.recording) && session.ended.is_none() =>
            {
                if let Some(call) = session.stop {
                    self.session = None;
                    return JobRoute::Reply(call, transcript_reply(Some(&job)));
                }
                session.ended = Some((Some(job), Instant::now() + STOP_GRACE));
                JobRoute::Held
            }
            _ => JobRoute::Native(job),
        }
    }

    /// The plugin asks for the transcript: the reply, or `None` to answer
    /// when it arrives.
    fn stop(&mut self, id: SessionId, call: C) -> Option<CoreReply> {
        let session = self.session.as_mut().filter(|session| session.id == id)?;
        if let Some((job, _)) = session.ended.take() {
            self.session = None;
            return Some(transcript_reply(job.as_ref()));
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

    /// The recording of an open session that has no transcript yet.
    fn awaiting_recording(&self) -> Option<RecordingId> {
        self.session
            .as_ref()
            .filter(|session| session.ended.is_none())
            .map(|session| session.recording)
    }

    /// The session's recording ended with nothing to transcribe.
    fn ended_without_transcript(&mut self, now: Instant) -> Option<(C, CoreReply)> {
        let session = self.session.as_mut()?;
        if let Some(call) = session.stop {
            self.session = None;
            return Some((call, transcript_reply(None)));
        }
        session.ended = Some((None, now + STOP_GRACE));
        None
    }

    /// A host stopped: its session is the core's again.
    fn host_stopped(&mut self, host: H) -> Option<DeliveryJob> {
        let session = self.session.take_if(|session| session.host == host)?;
        session.ended.and_then(|(job, _)| job)
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.session
            .as_ref()
            .and_then(|session| session.ended.as_ref())
            .map(|(_, until)| *until)
    }

    /// A transcript the plugin did not ask for in time.
    fn expire(&mut self, now: Instant) -> Option<DeliveryJob> {
        if self.next_deadline().is_some_and(|until| until <= now) {
            return self.session.take().and_then(|session| session.ended?.0);
        }
        None
    }
}

fn transcript_reply(job: Option<&DeliveryJob>) -> CoreReply {
    CoreReply::Transcript {
        text: job
            .and_then(|job| job.raw.as_ref())
            .map_or_else(String::new, |raw| raw.as_str().to_owned()),
        focus: job.map_or(FocusSnapshot::Unavailable, |job| {
            job.stop_context.focus.clone()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: u8 = 1;
    const FIRST: RecordingId = RecordingId::new(1);
    const SECOND: RecordingId = RecordingId::new(2);

    fn job(recording: RecordingId, text: &str) -> DeliveryJob {
        DeliveryJob {
            recording: Some(recording),
            raw: (!text.is_empty()).then(|| RawTranscript::new(text)),
            stop_context: StopContext {
                focus: FocusSnapshot::Unavailable,
                delivery: DeliveryTarget::Insert,
            },
        }
    }

    fn text(reply: &CoreReply) -> &str {
        match reply {
            CoreReply::Transcript { text, .. } => text,
            other @ (CoreReply::Session { .. }
            | CoreReply::Focus { .. }
            | CoreReply::Delivered { .. }
            | CoreReply::Done) => panic!("expected a transcript, got {other:?}"),
        }
    }

    fn native_text(route: JobRoute<u32>) -> String {
        match route {
            JobRoute::Native(job) => job
                .raw
                .map(|raw| raw.as_str().to_owned())
                .unwrap_or_default(),
            JobRoute::Reply(..) | JobRoute::Held | JobRoute::Dropped => {
                panic!("expected the built-in flow to get the job")
            }
        }
    }

    /// A press of `FIRST` at `at`, claimed `after` later.
    fn claim(sessions: &mut Sessions<u8, u32>, at: Instant, after: Duration) -> Open {
        sessions.pressed(FIRST, at);
        sessions.open(HOST, Some(FIRST), false, at + after)
    }

    #[test]
    fn a_press_is_claimable_only_within_the_window() {
        let at = Instant::now();
        let mut sessions = Sessions::<u8, u32>::default();
        assert!(matches!(
            claim(&mut sessions, at, CLAIM_WINDOW),
            Open::Adopted(_)
        ));

        let mut sessions = Sessions::<u8, u32>::default();
        let late = claim(&mut sessions, at, CLAIM_WINDOW + Duration::from_millis(1));
        assert!(matches!(late, Open::Refused(why) if why.contains("too late")));
        // The core keeps the late press.
        assert_eq!(native_text(sessions.job(job(FIRST, "hello"))), "hello");
    }

    #[test]
    fn opening_while_busy_without_a_press_is_refused_and_idle_starts() {
        let mut sessions = Sessions::<u8, u32>::default();
        let now = Instant::now();
        assert!(matches!(
            sessions.open(HOST, None, false, now),
            Open::Refused(_)
        ));
        assert!(matches!(sessions.open(HOST, None, true, now), Open::Start));
        // A press whose recording is no longer current is not claimable.
        sessions.pressed(FIRST, now);
        assert!(matches!(
            sessions.open(HOST, Some(SECOND), false, now),
            Open::Refused(_)
        ));
    }

    #[test]
    fn the_adopted_transcript_answers_stop_whichever_comes_first() {
        let at = Instant::now();
        // Stop first: answered when the transcript arrives.
        let mut sessions = Sessions::<u8, u32>::default();
        let Open::Adopted(id) = claim(&mut sessions, at, Duration::ZERO) else {
            panic!("should adopt");
        };
        assert!(sessions.stop(id, 7).is_none());
        let JobRoute::Reply(7, reply) = sessions.job(job(FIRST, "hello")) else {
            panic!("the waiting stop should be answered");
        };
        assert_eq!(text(&reply), "hello");

        // Transcript first: held, and stop answers at once.
        let mut sessions = Sessions::<u8, u32>::default();
        let Open::Adopted(id) = claim(&mut sessions, at, Duration::ZERO) else {
            panic!("should adopt");
        };
        assert!(matches!(sessions.job(job(FIRST, "later")), JobRoute::Held));
        assert_eq!(text(&sessions.stop(id, 8).expect("should answer")), "later");
        // The session is over; the next recording is the core's.
        assert_eq!(native_text(sessions.job(job(SECOND, "next"))), "next");
    }

    #[test]
    fn an_older_recordings_transcript_stays_with_the_core() {
        let mut sessions = Sessions::<u8, u32>::default();
        sessions.pressed(SECOND, Instant::now());
        assert!(matches!(
            sessions.open(HOST, Some(SECOND), false, Instant::now()),
            Open::Adopted(_)
        ));
        assert_eq!(native_text(sessions.job(job(FIRST, "old"))), "old");
        assert!(matches!(sessions.job(job(SECOND, "new")), JobRoute::Held));
    }

    #[test]
    fn the_core_delivers_what_a_dead_or_silent_plugin_owned() {
        let at = Instant::now();
        // The host dies holding the transcript.
        let mut sessions = Sessions::<u8, u32>::default();
        let _ = claim(&mut sessions, at, Duration::ZERO);
        assert!(matches!(sessions.job(job(FIRST, "held")), JobRoute::Held));
        assert!(
            sessions.host_stopped(2).is_none(),
            "another host owns nothing"
        );
        let job = sessions.host_stopped(HOST).expect("the core takes it back");
        assert_eq!(
            job.raw.map(|raw| raw.as_str().to_owned()).as_deref(),
            Some("held")
        );

        // The host dies before the transcript arrives.
        let mut sessions = Sessions::<u8, u32>::default();
        let _ = claim(&mut sessions, at, Duration::ZERO);
        assert!(sessions.host_stopped(HOST).is_none());
        assert_eq!(
            native_text(sessions.job(super::tests::job(FIRST, "after"))),
            "after"
        );

        // The plugin never asks.
        let mut sessions = Sessions::<u8, u32>::default();
        let _ = claim(&mut sessions, at, Duration::ZERO);
        assert!(matches!(
            sessions.job(super::tests::job(FIRST, "unasked")),
            JobRoute::Held
        ));
        assert!(sessions.expire(Instant::now()).is_none());
        let expired = sessions
            .expire(Instant::now() + STOP_GRACE + Duration::from_millis(1))
            .expect("the core delivers it after the grace period");
        assert_eq!(
            expired.raw.map(|raw| raw.as_str().to_owned()).as_deref(),
            Some("unasked")
        );
    }

    #[test]
    fn cancelled_and_empty_recordings_end_the_session() {
        let at = Instant::now();
        let mut sessions = Sessions::<u8, u32>::default();
        let Open::Adopted(id) = claim(&mut sessions, at, Duration::ZERO) else {
            panic!("should adopt");
        };
        sessions.cancel(id);
        assert!(matches!(
            sessions.job(job(FIRST, "dropped")),
            JobRoute::Dropped
        ));

        let mut sessions = Sessions::<u8, u32>::default();
        let Open::Adopted(id) = claim(&mut sessions, at, Duration::ZERO) else {
            panic!("should adopt");
        };
        assert!(sessions.stop(id, 3).is_none());
        let (call, reply) = sessions
            .ended_without_transcript(Instant::now())
            .expect("the waiting stop is answered");
        assert_eq!((call, text(&reply)), (3, ""));
        assert!(matches!(
            sessions.open(HOST, None, true, Instant::now()),
            Open::Start
        ));
    }
}
