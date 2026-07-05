use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
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
use crate::models::ModelCatalogEntry;
use crate::settings;
use crate::text::DictationContext;
use crate::text::DictationFormatter;
use crate::transcription::TranscriptionResult;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const CLIENT_READ_TIMEOUT: Duration = Duration::from_secs(2);
const SOCKET_FILE_NAME: &str = "dictate.sock";
const UNAVAILABLE_MESSAGE: &str = "transcription is unavailable; restart `dictate daemon`";

fn socket_path() -> Result<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;
    Ok(runtime_dir.join(SOCKET_FILE_NAME))
}

pub fn send(command: DictationCommand) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path()?)
        .map_err(|error| anyhow!("failed to connect to running Dictate daemon: {error}"))?;
    serde_json::to_writer(&mut stream, &command)?;
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

struct Daemon {
    socket: DaemonSocket,
    overlay: Overlay,
    dictation: DictationControl,
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

            loop {
                let command = match self.socket.accept() {
                    Ok(Some(command)) => command,
                    Ok(None) => continue,
                    Err(error) => {
                        eprintln!("failed to read record command: {error:#}");
                        continue;
                    }
                };

                match self.dictation.apply(command) {
                    DictationUpdate::Started => {
                        if self.dictation.phase() == DictationPhase::Unavailable {
                            eprintln!("{UNAVAILABLE_MESSAGE}");
                        } else {
                            eprintln!("opening microphone for dictation");
                        }
                    }
                    DictationUpdate::Stopped => {
                        self.overlay.hide();
                        eprintln!("dictation stopped; transcribing captured audio");
                    }
                    DictationUpdate::Cancelled => {
                        self.overlay.hide();
                        eprintln!("dictation cancelled");
                    }
                    DictationUpdate::Ignored(reason) => {
                        eprintln!("record command ignored: {reason}")
                    }
                    DictationUpdate::Busy(DictationPhase::Unavailable) => {
                        eprintln!("{UNAVAILABLE_MESSAGE}");
                    }
                    DictationUpdate::Busy(phase) => {
                        eprintln!("cannot change recording while {}", phase.label());
                    }
                }
            }
        });
    }

    fn spawn_microphone_worker(&self) {
        let dictation = self.dictation.clone();
        let overlay = self.overlay.clone();
        let model = self.model;
        let context = self.context.clone();
        let delivery = self.delivery;

        thread::spawn(move || {
            let result = || -> Result<()> {
                let model_dir = model.ensure_downloaded()?;
                let recognizer = model.create_recognizer(&model_dir)?;
                let formatter = DictationFormatter;
                let mut mic = None;
                dictation.mark_ready();
                eprintln!("transcription ready; run `dictate record start` to start dictation");

                loop {
                    thread::sleep(POLL_INTERVAL);

                    match mic_session_action(dictation.phase(), mic.is_some()) {
                        MicSessionAction::Open => {
                            let opened_mic =
                                crate::mic::capture(dictation.clone(), overlay.clone())?;
                            if dictation.phase() == DictationPhase::Recording {
                                mic = Some(opened_mic);
                                overlay.show();
                                if dictation.phase() == DictationPhase::Recording {
                                    eprintln!(
                                        "dictation started; run `dictate record stop` to transcribe"
                                    );
                                } else {
                                    overlay.hide();
                                    mic = None;
                                }
                            }
                        }
                        MicSessionAction::Close => {
                            mic = None;
                        }
                        MicSessionAction::Keep => {}
                    }

                    let Some(utterance) = dictation.take_utterance() else {
                        continue;
                    };
                    mic = None;

                    match crate::transcription::transcribe(&recognizer, &utterance) {
                        TranscriptionResult::Transcript(raw) => {
                            let text = formatter.format(raw, &context);
                            if !text.is_empty() {
                                delivery::deliver(delivery, text.as_str());
                            }
                        }
                        TranscriptionResult::NoTranscript(reason) => {
                            eprintln!("{}", reason.message())
                        }
                    }

                    overlay.hide();
                    dictation.finish_transcription();
                }
            }();

            if let Err(error) = result {
                eprintln!("transcription failed: {error:#}");
                overlay.hide();
                dictation.mark_unavailable();
            }
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MicSessionAction {
    Open,
    Close,
    Keep,
}

fn mic_session_action(phase: DictationPhase, is_open: bool) -> MicSessionAction {
    match (phase, is_open) {
        (DictationPhase::Recording, false) => MicSessionAction::Open,
        (DictationPhase::Recording, true) => MicSessionAction::Keep,
        (DictationPhase::Initializing, true)
        | (DictationPhase::Idle, true)
        | (DictationPhase::Transcribing, true)
        | (DictationPhase::Unavailable, true) => MicSessionAction::Close,
        (DictationPhase::Initializing, false)
        | (DictationPhase::Idle, false)
        | (DictationPhase::Transcribing, false)
        | (DictationPhase::Unavailable, false) => MicSessionAction::Keep,
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

    fn accept(&self) -> Result<Option<DictationCommand>> {
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
        let _ = fs::remove_file(&self.path);
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
    fn reads_record_commands() {
        for (wire_command, command) in [
            ("\"start\"\n", DictationCommand::Start),
            ("\"stop\"\n", DictationCommand::Stop),
            ("\"toggle\"\n", DictationCommand::Toggle),
            ("\"cancel\"\n", DictationCommand::Cancel),
        ] {
            assert_eq!(
                serde_json::from_str::<DictationCommand>(wire_command.trim()).ok(),
                Some(command)
            );
        }
    }

    #[test]
    fn ignores_unknown_command() {
        assert!(serde_json::from_str::<DictationCommand>("\"bogus\"").is_err());
    }

    #[test]
    fn mic_session_action_tracks_phase_and_open_state() {
        assert_eq!(
            mic_session_action(DictationPhase::Recording, false),
            MicSessionAction::Open
        );
        assert_eq!(
            mic_session_action(DictationPhase::Recording, true),
            MicSessionAction::Keep
        );
        assert_eq!(
            mic_session_action(DictationPhase::Transcribing, true),
            MicSessionAction::Close
        );
        assert_eq!(
            mic_session_action(DictationPhase::Idle, false),
            MicSessionAction::Keep
        );
    }

    #[test]
    fn slow_client_does_not_block_accept_loop() {
        let path = socket_test_path("slow-client");
        let socket =
            DaemonSocket::bind_at_with_read_timeout(path.clone(), Duration::from_millis(50))
                .unwrap();
        let mut client = UnixStream::connect(path).unwrap();
        client.write_all(b"\"sta").unwrap();

        let started = Instant::now();
        assert_eq!(socket.accept().unwrap(), None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn ignores_empty_clients() {
        let path = socket_test_path("empty-client");
        let socket = DaemonSocket::bind_at(path.clone()).unwrap();
        drop(UnixStream::connect(path).unwrap());

        assert_eq!(socket.accept().unwrap(), None);
    }

    #[test]
    fn reclaims_stale_socket_path() {
        let path = socket_test_path("stale");
        let stale_listener = UnixListener::bind(&path).unwrap();
        drop(stale_listener);

        let socket = DaemonSocket::bind_at(path.clone()).unwrap();

        assert!(path.exists());
        drop(socket);
        assert!(!path.exists());
    }
}
