use std::fs;
use std::io;
use std::io::Read;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use rustix::event::PollFd;
use rustix::event::PollFlags;
use rustix::event::Timespec;
use rustix::event::poll;
use rustix::io::Errno;
use rustix::net::AddressFamily;
use rustix::net::SocketAddrUnix;
use rustix::net::SocketFlags;
use rustix::net::SocketType;
use rustix::net::connect;
use rustix::net::socket_with;
use rustix::net::sockopt::socket_error;
use rustix::net::sockopt::socket_peercred;
use serde::Deserialize;

use crate::focus::FocusObservation;
use crate::focus::FocusProbeFailure;
use crate::focus::FocusProbeFailureKind;
use crate::focus::FocusProbeIoOperation;
use crate::focus::FocusProbeMessage;
use crate::focus::FocusResponseFailureKind;
use crate::focus::FocusSource;
use crate::focus::FocusedWindow;
use crate::focus::NiriInstanceId;
use crate::focus::SessionEnvironment;

const SOURCE: FocusSource = FocusSource::Niri;
const IPC_TIMEOUT: Duration = Duration::from_millis(200);
const RESPONSE_LIMIT: usize = 64 * 1024;
const REQUEST: &[u8] = b"\"FocusedWindow\"\n";

pub(super) fn observe(environment: &SessionEnvironment) -> FocusObservation {
    let Some(socket) = environment.niri_socket() else {
        return failure(FocusProbeFailureKind::EnvironmentUnavailable {
            variable: "NIRI_SOCKET",
        });
    };

    observe_socket(Path::new(socket))
}

fn observe_socket(socket: &Path) -> FocusObservation {
    let deadline = Instant::now() + IPC_TIMEOUT;
    let mut stream = match connect_socket(socket, deadline) {
        Ok(stream) => stream,
        Err(kind) => return failure(kind),
    };
    let instance = match compositor_instance(&stream) {
        Ok(instance) => instance,
        Err(kind) => return failure(kind),
    };
    if let Err(kind) = write_request(&mut stream, deadline) {
        return failure(kind);
    }

    match read_response(&mut stream, deadline) {
        Ok(response) => parse_response(&response, instance),
        Err(kind) => failure(kind),
    }
}

fn connect_socket(path: &Path, deadline: Instant) -> Result<UnixStream, FocusProbeFailureKind> {
    let address = SocketAddrUnix::new(path)
        .map_err(|error| rustix_io_failure(FocusProbeIoOperation::BuildSocketAddress, error))?;
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .map_err(|error| rustix_io_failure(FocusProbeIoOperation::CreateSocket, error))?;

    match connect(&socket, &address) {
        Ok(()) | Err(Errno::ISCONN) => {}
        Err(Errno::INPROGRESS | Errno::ALREADY | Errno::AGAIN) => {
            wait_until_ready(
                socket.as_fd(),
                PollFlags::OUT,
                deadline,
                FocusProbeIoOperation::Connect,
            )?;
            match socket_error(&socket)
                .map_err(|error| rustix_io_failure(FocusProbeIoOperation::CheckConnection, error))?
            {
                Ok(()) => {}
                Err(error) => {
                    return Err(rustix_io_failure(FocusProbeIoOperation::Connect, error));
                }
            }
        }
        Err(error) => return Err(rustix_io_failure(FocusProbeIoOperation::Connect, error)),
    }

    Ok(UnixStream::from(socket))
}

fn compositor_instance(stream: &UnixStream) -> Result<NiriInstanceId, FocusProbeFailureKind> {
    let credentials = socket_peercred(stream)
        .map_err(|error| rustix_io_failure(FocusProbeIoOperation::ReadPeerIdentity, error))?;
    let compositor_pid = credentials.pid.as_raw_pid();
    let stat = fs::read_to_string(format!("/proc/{compositor_pid}/stat"))
        .map_err(|error| std_io_failure(FocusProbeIoOperation::ReadPeerIdentity, &error))?;
    let start_time_ticks =
        parse_process_start_time(&stat).ok_or(FocusProbeFailureKind::InvalidPeerIdentity)?;
    Ok(NiriInstanceId::new(compositor_pid, start_time_ticks))
}

fn parse_process_start_time(stat: &str) -> Option<u64> {
    let command_end = stat.rfind(')')?;
    stat.get(command_end + 1..)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn write_request(stream: &mut UnixStream, deadline: Instant) -> Result<(), FocusProbeFailureKind> {
    let mut written = 0;
    while written < REQUEST.len() {
        ensure_before_deadline(deadline, FocusProbeIoOperation::WriteRequest)?;
        match stream.write(&REQUEST[written..]) {
            Ok(0) => {
                return Err(FocusProbeFailureKind::Io {
                    operation: FocusProbeIoOperation::WriteRequest,
                    kind: io::ErrorKind::WriteZero,
                });
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => wait_until_ready(
                stream.as_fd(),
                PollFlags::OUT,
                deadline,
                FocusProbeIoOperation::WriteRequest,
            )?,
            Err(error) => {
                return Err(std_io_failure(FocusProbeIoOperation::WriteRequest, &error));
            }
        }
    }
    Ok(())
}

fn read_response<Stream: Read + AsFd>(
    stream: &mut Stream,
    deadline: Instant,
) -> Result<Vec<u8>, FocusProbeFailureKind> {
    let mut response = Vec::new();
    let mut buffer = [0_u8; 1024];

    loop {
        ensure_before_deadline(deadline, FocusProbeIoOperation::ReadResponse)?;
        let read = match stream.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_until_ready(
                    stream.as_fd(),
                    PollFlags::IN,
                    deadline,
                    FocusProbeIoOperation::ReadResponse,
                )?;
                continue;
            }
            Err(error) => {
                return Err(std_io_failure(FocusProbeIoOperation::ReadResponse, &error));
            }
        };
        if read == 0 {
            break;
        }

        let received = &buffer[..read];
        let line_end = received.iter().position(|byte| *byte == b'\n');
        let payload = line_end.map_or(received, |line_end| &received[..line_end]);
        if response.len().saturating_add(payload.len()) > RESPONSE_LIMIT {
            return Err(FocusProbeFailureKind::ResponseTooLarge {
                limit: RESPONSE_LIMIT,
            });
        }
        response.extend_from_slice(payload);
        if line_end.is_some() {
            break;
        }
    }

    Ok(response)
}

fn wait_until_ready(
    fd: std::os::fd::BorrowedFd<'_>,
    interest: PollFlags,
    deadline: Instant,
    operation: FocusProbeIoOperation,
) -> Result<(), FocusProbeFailureKind> {
    loop {
        let remaining = remaining(deadline, operation)?;
        let timeout =
            Timespec::try_from(remaining).map_err(|_error| FocusProbeFailureKind::Io {
                operation: FocusProbeIoOperation::Poll,
                kind: io::ErrorKind::InvalidInput,
            })?;
        let mut poll_fds = [PollFd::from_borrowed_fd(fd, interest)];
        match poll(&mut poll_fds, Some(&timeout)) {
            Ok(0) => return Err(timed_out(operation)),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => {}
            Err(error) => {
                return Err(rustix_io_failure(FocusProbeIoOperation::Poll, error));
            }
        }
    }
}

fn ensure_before_deadline(
    deadline: Instant,
    operation: FocusProbeIoOperation,
) -> Result<(), FocusProbeFailureKind> {
    remaining(deadline, operation).map(|_remaining| ())
}

fn remaining(
    deadline: Instant,
    operation: FocusProbeIoOperation,
) -> Result<Duration, FocusProbeFailureKind> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| timed_out(operation))
}

fn timed_out(operation: FocusProbeIoOperation) -> FocusProbeFailureKind {
    FocusProbeFailureKind::TimedOut {
        operation,
        timeout: IPC_TIMEOUT,
    }
}

fn failure(kind: FocusProbeFailureKind) -> FocusObservation {
    FocusObservation::ProbeFailed(FocusProbeFailure::new(SOURCE, kind))
}

#[derive(Deserialize)]
enum NiriResponse {
    FocusedWindow(Option<NiriFocusedWindow>),
}

type NiriReply = Result<NiriResponse, String>;

#[derive(Deserialize)]
struct NiriFocusedWindow {
    id: u64,
    app_id: Option<String>,
    title: Option<String>,
}

fn parse_response(response: &[u8], instance: NiriInstanceId) -> FocusObservation {
    let reply: NiriReply = match serde_json::from_slice(response) {
        Ok(reply) => reply,
        Err(error) => {
            return failure(FocusProbeFailureKind::InvalidResponse {
                kind: FocusResponseFailureKind::from_json_category(error.classify()),
                line: error.line(),
                column: error.column(),
            });
        }
    };

    match reply {
        Ok(NiriResponse::FocusedWindow(Some(window))) => {
            FocusObservation::Focused(FocusedWindow::niri(
                instance,
                window.id,
                window.app_id.as_deref(),
                window.title.as_deref(),
            ))
        }
        Ok(NiriResponse::FocusedWindow(None)) => {
            FocusObservation::NoFocusedWindow { source: SOURCE }
        }
        Err(message) => failure(FocusProbeFailureKind::RequestRejected {
            message: FocusProbeMessage::new(&message),
        }),
    }
}

fn std_io_failure(operation: FocusProbeIoOperation, error: &io::Error) -> FocusProbeFailureKind {
    FocusProbeFailureKind::Io {
        operation,
        kind: error.kind(),
    }
}

fn rustix_io_failure(operation: FocusProbeIoOperation, error: Errno) -> FocusProbeFailureKind {
    FocusProbeFailureKind::Io {
        operation,
        kind: io::Error::from_raw_os_error(error.raw_os_error()).kind(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    use std::thread;

    use super::*;

    // The counter only prevents same-process fixture name collisions; it guards no shared data.
    static SOCKET_ID: AtomicU64 = AtomicU64::new(0);

    fn test_socket_path(name: &str) -> std::path::PathBuf {
        let id = SOCKET_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "dictate-focus-{name}-{}-{id}.sock",
            std::process::id()
        ))
    }

    fn test_instance() -> NiriInstanceId {
        NiriInstanceId::new(10, 100)
    }

    #[test]
    fn parses_process_start_time_after_parenthesized_command() {
        let stat = "42 (niri compositor) S 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 999";

        assert_eq!(parse_process_start_time(stat), Some(999));
        assert_eq!(parse_process_start_time("malformed"), None);
    }

    #[test]
    fn queries_niri_socket_with_newline_framing() {
        let path = test_socket_path("request");
        let listener = UnixListener::bind(&path).expect("test socket should bind");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("test client should connect");
            let mut request = [0_u8; REQUEST.len()];
            stream
                .read_exact(&mut request)
                .expect("test request should arrive");
            assert_eq!(&request, REQUEST);
            stream
                .write_all(
                    br#"{"Ok":{"FocusedWindow":{"id":42,"title":"Editor","app_id":"dev.editor"}}}"#,
                )
                .expect("test response should be written");
            stream
                .write_all(b"\n")
                .expect("test response newline should be written");
        });

        let observation = observe_socket(&path);
        server.join().expect("test server should finish");
        std::fs::remove_file(&path).expect("test socket should be removed");

        assert_eq!(
            observation.to_string(),
            "dev.editor — Editor (reported by niri)"
        );
    }

    #[test]
    fn stalled_niri_socket_is_bounded_by_attempt_deadline() {
        let path = test_socket_path("timeout");
        let listener = UnixListener::bind(&path).expect("test socket should bind");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("test client should connect");
            let mut request = [0_u8; REQUEST.len()];
            stream
                .read_exact(&mut request)
                .expect("test request should arrive");
            thread::sleep(IPC_TIMEOUT.saturating_mul(2));
        });

        let started = Instant::now();
        let observation = observe_socket(&path);
        let elapsed = started.elapsed();
        server.join().expect("test server should finish");
        std::fs::remove_file(&path).expect("test socket should be removed");

        assert!(elapsed < Duration::from_secs(1));
        assert_read_timeout(&observation);
    }

    #[test]
    fn drip_fed_niri_response_cannot_extend_attempt_deadline() {
        let path = test_socket_path("drip");
        let listener = UnixListener::bind(&path).expect("test socket should bind");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("test client should connect");
            let mut request = [0_u8; REQUEST.len()];
            stream
                .read_exact(&mut request)
                .expect("test request should arrive");
            for byte in b"{\"Ok\":" {
                if stream.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(80));
            }
        });

        let started = Instant::now();
        let observation = observe_socket(&path);
        let elapsed = started.elapsed();
        server.join().expect("test server should finish");
        std::fs::remove_file(&path).expect("test socket should be removed");

        assert!(elapsed < Duration::from_secs(1));
        assert_read_timeout(&observation);
    }

    fn assert_read_timeout(observation: &FocusObservation) {
        assert!(matches!(
            observation,
            FocusObservation::ProbeFailed(FocusProbeFailure {
                kind: FocusProbeFailureKind::TimedOut {
                    operation: FocusProbeIoOperation::ReadResponse,
                    timeout: IPC_TIMEOUT,
                },
                ..
            })
        ));
    }

    #[test]
    fn parses_focused_window_without_leaking_niri_response_shape() {
        let response = r#"{"Ok":{"FocusedWindow":{
            "id": 2,
            "title": "README.md — dictate",
            "app_id": "dev.zed.Zed",
            "is_focused": true,
            "layout": {"window_size": [1200, 800]}
        }}}"#;

        let observation = parse_response(response.as_bytes(), test_instance());

        assert_eq!(
            observation.to_string(),
            "dev.zed.Zed — README.md — dictate (reported by niri)"
        );
    }

    #[test]
    fn parses_null_as_no_focused_window() {
        assert_eq!(
            parse_response(br#"{"Ok":{"FocusedWindow":null}}"#, test_instance()),
            FocusObservation::NoFocusedWindow { source: SOURCE }
        );
    }

    #[test]
    fn malformed_response_is_a_typed_probe_failure_without_raw_json() {
        let observation = parse_response(br#"{"secret":"do not log me"}"#, test_instance());

        let failure = match observation {
            FocusObservation::ProbeFailed(failure) => failure,
            FocusObservation::Focused(_)
            | FocusObservation::NoFocusedWindow { .. }
            | FocusObservation::UnsupportedSession => {
                panic!("expected malformed response to fail")
            }
        };
        let rendered = failure.to_string();
        assert!(rendered.contains("invalid JSON response"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("do not log me"));
    }

    #[test]
    fn rejected_request_message_is_safe_for_one_line_logs() {
        let observation = parse_response(br#"{"Err":"bad\n\u001b[31mrequest"}"#, test_instance());

        assert_eq!(
            observation.to_string(),
            "unavailable (niri focus probe failed: request rejected: bad\\u{a}\\u{1b}[31mrequest)"
        );
    }

    #[test]
    fn response_reader_stops_at_newline() {
        let (mut reader, mut writer) = UnixStream::pair().expect("socket pair should be created");
        writer
            .write_all(b"reply\nignored")
            .expect("response fixture should be written");
        reader
            .set_nonblocking(true)
            .expect("test reader should become nonblocking");

        assert_eq!(
            read_response(&mut reader, Instant::now() + IPC_TIMEOUT),
            Ok(b"reply".to_vec())
        );
    }

    #[test]
    fn response_reader_rejects_oversized_payload() {
        let response = vec![b'x'; RESPONSE_LIMIT + 1];
        let (mut reader, mut writer) = UnixStream::pair().expect("socket pair should be created");
        let writer = thread::spawn(move || {
            writer
                .write_all(&response)
                .expect("response fixture should be written");
        });
        reader
            .set_nonblocking(true)
            .expect("test reader should become nonblocking");

        assert_eq!(
            read_response(&mut reader, Instant::now() + Duration::from_secs(1)),
            Err(FocusProbeFailureKind::ResponseTooLarge {
                limit: RESPONSE_LIMIT
            })
        );
        writer.join().expect("fixture writer should finish");
    }
}
