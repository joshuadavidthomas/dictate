use std::io;
use std::os::fd::AsFd as _;
use std::os::unix::process::ExitStatusExt as _;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use rustix::event::poll;
use rustix::fs::OFlags;
use rustix::fs::fcntl_getfl;
use rustix::fs::fcntl_setfl;
use rustix::io::Errno;

use super::ClipboardPasteChord;
use super::InsertionText;
use super::WtypeExitStatus;
use super::WtypeFailure;

const WTYPE_TIMEOUT: Duration = Duration::from_secs(2);
const TERMINATION_WAIT: Duration = Duration::from_millis(250);
const WAIT_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum WtypeOutcome {
    Completed {
        input_bytes: usize,
    },
    NotStarted(WtypeFailure),
    DeliveryUncertain {
        maybe_input_bytes: usize,
        failure: WtypeFailure,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ClipboardPasteChordOutcome {
    Sent,
    NotSent(WtypeFailure),
    DeliveryUncertain(WtypeFailure),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ClipboardPasteChordBackend;

impl ClipboardPasteChord for ClipboardPasteChordBackend {
    fn send_clipboard_paste_chord(&mut self) -> ClipboardPasteChordOutcome {
        send_clipboard_paste_chord(clipboard_paste_command())
    }
}

pub(super) fn type_text(text: InsertionText<'_>) -> WtypeOutcome {
    type_text_with_command(Command::new("wtype"), text)
}

fn type_text_with_command(command: Command, text: InsertionText<'_>) -> WtypeOutcome {
    type_text_with_command_timeout(command, text, WTYPE_TIMEOUT)
}

fn type_text_with_command_timeout(
    mut command: Command,
    text: InsertionText<'_>,
    timeout: Duration,
) -> WtypeOutcome {
    let deadline = Instant::now() + timeout;
    let mut child = match command.arg("-").stdin(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(error) => return WtypeOutcome::NotStarted(spawn_failure(&error)),
    };

    let Some(mut stdin) = child.stdin.take() else {
        terminate(child);
        return WtypeOutcome::DeliveryUncertain {
            maybe_input_bytes: 0,
            failure: WtypeFailure::StdinUnavailable,
        };
    };

    if let Err(error) = make_nonblocking(&stdin) {
        terminate(child);
        return WtypeOutcome::DeliveryUncertain {
            maybe_input_bytes: 0,
            failure: WtypeFailure::WriteStdin {
                written_bytes: 0,
                kind: error.kind(),
                message: error.to_string(),
            },
        };
    }

    let input = text.as_str().as_bytes();
    let written = match write_until(&mut stdin, input, deadline) {
        Ok(written) => written,
        Err((written, failure)) => {
            drop(stdin);
            terminate(child);
            return WtypeOutcome::DeliveryUncertain {
                maybe_input_bytes: written,
                failure,
            };
        }
    };
    drop(stdin);

    match wait_until(&mut child, deadline) {
        Ok(status) if status.success() => WtypeOutcome::Completed {
            input_bytes: written,
        },
        Ok(status) => WtypeOutcome::DeliveryUncertain {
            maybe_input_bytes: written,
            failure: WtypeFailure::Exited {
                status: exit_status(status),
            },
        },
        Err(failure) => {
            terminate(child);
            WtypeOutcome::DeliveryUncertain {
                maybe_input_bytes: written,
                failure,
            }
        }
    }
}

fn clipboard_paste_command() -> Command {
    let mut command = Command::new("wtype");
    command.args([
        "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl",
    ]);
    command
}

fn send_clipboard_paste_chord(mut command: Command) -> ClipboardPasteChordOutcome {
    let deadline = Instant::now() + WTYPE_TIMEOUT;
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return ClipboardPasteChordOutcome::NotSent(spawn_failure(&error)),
    };

    match wait_until(&mut child, deadline) {
        Ok(status) if status.success() => ClipboardPasteChordOutcome::Sent,
        Ok(status) => ClipboardPasteChordOutcome::DeliveryUncertain(WtypeFailure::Exited {
            status: exit_status(status),
        }),
        Err(failure) => {
            terminate(child);
            ClipboardPasteChordOutcome::DeliveryUncertain(failure)
        }
    }
}

fn make_nonblocking(fd: &impl std::os::fd::AsFd) -> io::Result<()> {
    let flags = fcntl_getfl(fd).map_err(errno_to_io)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(errno_to_io)
}

fn write_until(
    writer: &mut (impl io::Write + std::os::fd::AsFd),
    input: &[u8],
    deadline: Instant,
) -> Result<usize, (usize, WtypeFailure)> {
    let mut written = 0;
    while written < input.len() {
        if deadline
            .checked_duration_since(Instant::now())
            .is_none_or(|remaining| remaining.is_zero())
        {
            return Err((written, WtypeFailure::TimedOut));
        }
        match writer.write(&input[written..]) {
            Ok(0) => {
                return Err((
                    written,
                    WtypeFailure::WriteStdin {
                        written_bytes: written,
                        kind: io::ErrorKind::WriteZero,
                        message: "wtype stdin accepted zero bytes".to_owned(),
                    },
                ));
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Err(failure) = wait_writable(writer.as_fd(), deadline) {
                    return Err((written, failure));
                }
            }
            Err(error) => {
                return Err((
                    written,
                    WtypeFailure::WriteStdin {
                        written_bytes: written,
                        kind: error.kind(),
                        message: error.to_string(),
                    },
                ));
            }
        }
    }
    Ok(written)
}

fn wait_writable(fd: std::os::fd::BorrowedFd<'_>, deadline: Instant) -> Result<(), WtypeFailure> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(WtypeFailure::TimedOut)?;
        let timeout = Timespec::try_from(remaining).map_err(|_error| WtypeFailure::Wait {
            kind: io::ErrorKind::InvalidInput,
            message: "invalid wtype poll timeout".to_owned(),
        })?;
        let mut poll_fds = [PollFd::from_borrowed_fd(fd, PollFlags::OUT)];
        match poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) => return Err(WtypeFailure::TimedOut),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => {}
            Err(error) => {
                let error = errno_to_io(error);
                return Err(WtypeFailure::Wait {
                    kind: error.kind(),
                    message: error.to_string(),
                });
            }
        }
    }
}

fn wait_until(child: &mut Child, deadline: Instant) -> Result<ExitStatus, WtypeFailure> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                return Err(WtypeFailure::Wait {
                    kind: error.kind(),
                    message: error.to_string(),
                });
            }
        }

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err(WtypeFailure::TimedOut);
        };
        if remaining.is_zero() {
            return Err(WtypeFailure::TimedOut);
        }
        thread::sleep(remaining.min(WAIT_INTERVAL));
    }
}

fn terminate(mut child: Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    drop(child.kill());

    let deadline = Instant::now() + TERMINATION_WAIT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => {}
        }

        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            drop(thread::spawn(move || drop(child.wait())));
            return;
        };
        if remaining.is_zero() {
            drop(thread::spawn(move || drop(child.wait())));
            return;
        }
        thread::sleep(remaining.min(WAIT_INTERVAL));
    }
}

fn spawn_failure(error: &io::Error) -> WtypeFailure {
    WtypeFailure::Spawn {
        kind: error.kind(),
        message: error.to_string(),
    }
}

fn exit_status(status: ExitStatus) -> WtypeExitStatus {
    if let Some(code) = status.code() {
        WtypeExitStatus::Code(code)
    } else if let Some(signal) = status.signal() {
        WtypeExitStatus::Signal(signal)
    } else {
        WtypeExitStatus::Unknown
    }
}

fn errno_to_io(error: Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;

    static TEST_ID: AtomicUsize = AtomicUsize::new(0);

    fn test_path(name: &str) -> std::path::PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("dictate-wtype-{name}-{}-{id}", std::process::id()))
    }

    #[test]
    fn direct_wtype_receives_arbitrary_text_on_stdin() {
        let output = test_path("stdin");
        let text = "--leading-dashes\nUTF-8: café\0trailing";
        let insertion_text = InsertionText::new(text).expect("fixture should be non-empty");
        let mut command = Command::new("sh");
        command
            .args(["-c", "test \"$2\" = - && cat > \"$1\"", "wtype-test"])
            .arg(&output);

        let outcome = type_text_with_command(command, insertion_text);

        assert_eq!(
            outcome,
            WtypeOutcome::Completed {
                input_bytes: text.len(),
            }
        );
        assert_eq!(
            fs::read(&output).expect("captured stdin should be readable"),
            text.as_bytes()
        );
        fs::remove_file(output).expect("captured stdin should be removed");
    }

    #[test]
    fn direct_spawn_failure_is_safe_to_report_as_not_started() {
        let command = Command::new(test_path("missing-program"));
        let text = InsertionText::new("hello").expect("fixture should be non-empty");

        let outcome = type_text_with_command(command, text);

        assert!(matches!(
            outcome,
            WtypeOutcome::NotStarted(WtypeFailure::Spawn {
                kind: io::ErrorKind::NotFound,
                ..
            })
        ));
    }

    #[test]
    fn hanging_direct_child_times_out_and_cleanup_returns() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let text = InsertionText::new("hello").expect("fixture should be non-empty");
        let started = Instant::now();

        let outcome = type_text_with_command_timeout(command, text, Duration::from_millis(50));

        assert_eq!(
            outcome,
            WtypeOutcome::DeliveryUncertain {
                maybe_input_bytes: 5,
                failure: WtypeFailure::TimedOut,
            }
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn clipboard_paste_command_is_exactly_one_ctrl_shift_v_chord() {
        let command = clipboard_paste_command();
        let args = command.get_args().collect::<Vec<_>>();

        assert_eq!(
            args,
            [
                "-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl",
            ]
            .map(OsStr::new)
            .as_slice()
        );
    }

    #[test]
    fn paste_spawn_failure_is_before_the_point_of_no_return() {
        let command = Command::new(test_path("missing-paste-program"));

        let outcome = send_clipboard_paste_chord(command);

        assert!(matches!(
            outcome,
            ClipboardPasteChordOutcome::NotSent(WtypeFailure::Spawn {
                kind: io::ErrorKind::NotFound,
                ..
            })
        ));
    }

    #[test]
    fn paste_nonzero_exit_is_uncertain_after_launch() {
        let mut command = Command::new("sh");
        command.args(["-c", "exit 23"]);

        let outcome = send_clipboard_paste_chord(command);

        assert_eq!(
            outcome,
            ClipboardPasteChordOutcome::DeliveryUncertain(WtypeFailure::Exited {
                status: WtypeExitStatus::Code(23),
            })
        );
    }
}
