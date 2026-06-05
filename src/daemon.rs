use std::fs;
use std::io::Read;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;

use crate::app::App;
use crate::dictation::ControlOutcome;
use crate::dictation::DictationCommand;
use crate::dictation::DictationControl;
use crate::dictation::DictationPhase;
use crate::models::default_model;
use crate::text::DictationContext;
use crate::text::DictationFormatter;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const SOCKET_FILE_NAME: &str = "dictate.sock";

pub fn send(command: DictationCommand) -> Result<()> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;
    let path = runtime_dir.join(SOCKET_FILE_NAME);
    let mut stream = UnixStream::connect(path)
        .map_err(|error| anyhow!("failed to connect to running Dictate daemon: {error}"))?;
    serde_json::to_writer(&mut stream, &command)?;
    stream.write_all(b"\n")?;
    Ok(())
}

pub fn run() -> Result<()> {
    Daemon::start()?.run()
}

struct Daemon {
    socket: DaemonSocket,
    app: App,
    dictation: DictationControl,
}

impl Daemon {
    fn start() -> Result<Self> {
        let daemon = Self {
            socket: DaemonSocket::bind()?,
            app: App::new(),
            dictation: DictationControl::new(),
        };
        daemon.spawn_microphone_worker();

        Ok(daemon)
    }

    fn run(self) -> Result<()> {
        eprintln!("Dictate daemon ready; run `dictate record toggle` to start dictation");

        loop {
            let Some(command) = self.socket.accept()? else {
                continue;
            };

            let outcome = match command {
                DictationCommand::Start => self.dictation.start_recording(),
                DictationCommand::Stop => self.dictation.stop_recording(),
                DictationCommand::Toggle => match self.dictation.phase() {
                    DictationPhase::Idle => self.dictation.start_recording(),
                    DictationPhase::Recording => self.dictation.stop_recording(),
                    DictationPhase::Transcribing | DictationPhase::Unavailable => {
                        ControlOutcome::Busy(self.dictation.phase())
                    }
                },
                DictationCommand::Cancel => self.dictation.cancel_recording(),
            };

            match outcome {
                ControlOutcome::Started => {
                    self.app.show();
                    eprintln!("dictation started; run `dictate record stop` to transcribe");
                }
                ControlOutcome::Stopped => {
                    eprintln!("dictation stopped; transcribing captured audio");
                    if self.dictation.phase() == DictationPhase::Idle {
                        self.app.hide();
                    }
                }
                ControlOutcome::Cancelled => {
                    self.app.hide();
                    eprintln!("dictation cancelled");
                }
                ControlOutcome::Ignored(reason) => eprintln!("record command ignored: {reason}"),
                ControlOutcome::Busy(phase) => {
                    eprintln!("cannot change recording while {}", phase.label());
                }
            }
        }
    }

    fn spawn_microphone_worker(&self) {
        let dictation = self.dictation.clone();
        let app = self.app.clone();

        thread::spawn(move || {
            let result = || -> Result<()> {
                let model = default_model();
                let model_dir = model.ensure_downloaded()?;
                let recognizer = model.create_recognizer(&model_dir)?;
                let formatter = DictationFormatter;
                let context = DictationContext::default();
                let _mic = crate::mic::capture(dictation.clone(), app.clone())?;
                eprintln!("microphone ready; run `dictate record start` to start dictation");

                loop {
                    thread::sleep(POLL_INTERVAL);

                    let Some(utterance) = dictation.take_utterance() else {
                        continue;
                    };

                    if crate::transcription::too_short_or_quiet(&utterance) {
                        eprintln!("captured dictation was too short or too quiet");
                    } else if let Some(raw) =
                        crate::transcription::transcribe(&recognizer, &utterance)
                        && !crate::transcription::transcript_is_noise(raw.as_str())
                    {
                        let text = formatter.format(raw, &context);
                        if !text.is_empty() {
                            println!("{}", text.as_str());
                        }
                    }

                    dictation.finish_transcription();
                    app.hide();
                }
            }();

            if let Err(error) = result {
                eprintln!("transcription failed: {error:#}");
                dictation.mark_unavailable();
            }
        });
    }
}

struct DaemonSocket {
    path: PathBuf,
    listener: UnixListener,
}

impl DaemonSocket {
    fn bind() -> Result<Self> {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;
        let path = runtime_dir.join(SOCKET_FILE_NAME);

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

        Ok(Self { path, listener })
    }

    fn accept(&self) -> Result<Option<DictationCommand>> {
        let (mut stream, _) = self.listener.accept()?;
        let mut command = String::new();
        stream.read_to_string(&mut command)?;

        match serde_json::from_str(command.trim()) {
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
    use super::*;

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
}
