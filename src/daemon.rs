use anyhow::Result;

use crate::app::App;
use crate::control::ControlCommand;
use crate::control::ControlListener;
use crate::transcription::DictationTranscriber;

pub fn run() -> Result<()> {
    let control = ControlListener::bind()?;
    let app = App::new();
    let transcriber = DictationTranscriber::start(app);

    eprintln!("Dictate daemon ready; run `dictate record toggle` to start dictation");

    loop {
        if let Some(command) = control.accept()? {
            handle_control_command(&transcriber, command);
        }
    }
}

fn handle_control_command(transcriber: &DictationTranscriber, command: ControlCommand) {
    match command {
        ControlCommand::Start => {
            transcriber.start_recording();
        }
        ControlCommand::Stop => {
            transcriber.stop_recording();
        }
        ControlCommand::Toggle => {
            transcriber.toggle();
        }
        ControlCommand::Cancel => {
            transcriber.cancel_recording();
        }
    }
}
