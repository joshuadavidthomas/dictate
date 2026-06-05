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

use crate::app::Overlay;
use crate::dictation::DictationCommand;
use crate::dictation::DictationControl;
use crate::dictation::DictationPhase;
use crate::dictation::DictationUpdate;
use crate::models::default_model;
use crate::text::DictationContext;
use crate::text::DictationFormatter;
use crate::transcription::TranscriptionResult;

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
    crate::app::run(|overlay| {
        Daemon::start(overlay)?.run_in_background();
        Ok(())
    })
}

struct Daemon {
    socket: DaemonSocket,
    overlay: Overlay,
    dictation: DictationControl,
}

impl Daemon {
    fn start(overlay: Overlay) -> Result<Self> {
        let daemon = Self {
            socket: DaemonSocket::bind()?,
            overlay,
            dictation: DictationControl::new(),
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
                        self.overlay.show();
                        eprintln!("dictation started; run `dictate record stop` to transcribe");
                    }
                    DictationUpdate::Stopped => {
                        eprintln!("dictation stopped; transcribing captured audio");
                        if self.dictation.phase() == DictationPhase::Idle {
                            self.overlay.hide();
                        }
                    }
                    DictationUpdate::Cancelled => {
                        self.overlay.hide();
                        eprintln!("dictation cancelled");
                    }
                    DictationUpdate::Ignored(reason) => {
                        eprintln!("record command ignored: {reason}")
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

        thread::spawn(move || {
            let result = || -> Result<()> {
                let model = default_model();
                let model_dir = model.ensure_downloaded()?;
                let recognizer = model.create_recognizer(&model_dir)?;
                let formatter = DictationFormatter;
                let context = DictationContext::default();
                let _mic = crate::mic::capture(dictation.clone(), overlay.clone())?;
                eprintln!("microphone ready; run `dictate record start` to start dictation");

                loop {
                    thread::sleep(POLL_INTERVAL);

                    let Some(utterance) = dictation.take_utterance() else {
                        continue;
                    };

                    match crate::transcription::transcribe(&recognizer, &utterance) {
                        TranscriptionResult::Transcript(raw) => {
                            let text = formatter.format(raw, &context);
                            if !text.is_empty() {
                                println!("{}", text.as_str());
                            }
                        }
                        TranscriptionResult::NoTranscript(reason) => {
                            eprintln!("{}", reason.message())
                        }
                    }

                    dictation.finish_transcription();
                    overlay.hide();
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
