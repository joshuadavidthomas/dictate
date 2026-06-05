use std::fs;
use std::io::Read;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::Result;
use anyhow::anyhow;

const SOCKET_FILE_NAME: &str = "dictate.sock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlCommand {
    Start,
    Stop,
    Toggle,
    Cancel,
}

impl ControlCommand {
    fn as_wire_command(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Toggle => "toggle",
            Self::Cancel => "cancel",
        }
    }
}

pub struct ControlListener {
    socket_path: PathBuf,
    listener: UnixListener,
}

impl ControlListener {
    pub fn bind() -> Result<Self> {
        let socket_path = socket_path()?;
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }
        remove_stale_socket(&socket_path)?;

        let listener = UnixListener::bind(&socket_path)?;

        Ok(Self {
            socket_path,
            listener,
        })
    }

    pub fn accept(&self) -> Result<Option<ControlCommand>> {
        let (mut stream, _) = self.listener.accept()?;
        Ok(read_command(&mut stream))
    }
}

impl Drop for ControlListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

pub fn send_command(command: ControlCommand) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path()?)
        .map_err(|error| anyhow!("failed to connect to running Dictate daemon: {error}"))?;
    writeln!(stream, "{}", command.as_wire_command())?;
    Ok(())
}

fn read_command(stream: &mut UnixStream) -> Option<ControlCommand> {
    let mut buffer = [0_u8; 64];
    let bytes = stream.read(&mut buffer).ok()?;
    let command = std::str::from_utf8(&buffer[..bytes]).ok()?.trim();

    match command {
        "start" => Some(ControlCommand::Start),
        "stop" => Some(ControlCommand::Stop),
        "toggle" => Some(ControlCommand::Toggle),
        "cancel" => Some(ControlCommand::Cancel),
        unknown => {
            eprintln!("unknown control command: {unknown}");
            None
        }
    }
}

fn remove_stale_socket(socket_path: &PathBuf) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }

    if UnixStream::connect(socket_path).is_ok() {
        return Err(anyhow!(
            "Dictate control socket is already in use at {}",
            socket_path.display()
        ));
    }

    fs::remove_file(socket_path)?;
    Ok(())
}

fn socket_path() -> Result<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;

    Ok(runtime_dir.join(SOCKET_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_record_commands() {
        for (wire_command, command) in [
            ("start\n", ControlCommand::Start),
            ("stop\n", ControlCommand::Stop),
            ("toggle\n", ControlCommand::Toggle),
            ("cancel\n", ControlCommand::Cancel),
        ] {
            let (mut left, mut right) = UnixStream::pair().expect("stream pair");
            left.write_all(wire_command.as_bytes())
                .expect("write command");

            assert_eq!(read_command(&mut right), Some(command));
        }
    }

    #[test]
    fn ignores_unknown_command() {
        let (mut left, mut right) = UnixStream::pair().expect("stream pair");
        left.write_all(b"bogus\n").expect("write command");

        assert_eq!(read_command(&mut right), None);
    }
}
