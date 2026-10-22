//! `dictate.exec`: run a command with a timeout the core enforces.
//!
//! Time spent waiting on the command does not count against the calling
//! hook's budget, and the daemon is told about the command so it can extend
//! its own deadline and kill the command if it kills the plugin host.

use std::io::Read;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use mlua::Lua;
use mlua::Table;
use mlua::Value;

use crate::Deadline;
use crate::MAX_HOOK_BUDGET;

/// How long a command may run when the plugin does not say.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of each output stream is kept; the rest is read and dropped.
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
/// How often a running command is checked for exit.
const EXIT_POLL: Duration = Duration::from_millis(5);

/// What the plugin host tells the daemon about commands it runs.
pub(crate) enum ExecEvent {
    /// A command started; the daemon may wait up to `timeout` longer and
    /// must kill process group `pid` if it kills the host.
    Started { pid: u32, timeout: Duration },
    /// The command and its process group are gone.
    Exited { pid: u32 },
}

/// Receives [`ExecEvent`]s; absent when plugins run in-process (tests).
pub(crate) struct ExecReporter(pub(crate) Box<dyn Fn(ExecEvent)>);

/// `dictate.exec({ program, args... }, { timeout_ms, stdin, cwd })`.
///
/// Returns `{ code, stdout, stderr, timed_out }`, where `code` is nil when
/// the command was killed, or `nil, message` when it could not start.
pub(crate) fn exec(
    lua: &Lua,
    (command_line, options): (Vec<String>, Option<Table>),
) -> mlua::Result<(Value, Value)> {
    let Some((program, args)) = command_line.split_first() else {
        return Err(mlua::Error::runtime("dictate.exec needs a program name"));
    };
    let ExecOptions {
        timeout,
        stdin,
        cwd,
    } = ExecOptions::parse(options.as_ref())?;

    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    // Its own process group, so a timeout also reaches what it started.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);

    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = format!("could not run {program}: {error}");
            return Ok((Value::Nil, Value::String(lua.create_string(message)?)));
        }
    };
    let pid = child.id();
    report(lua, ExecEvent::Started { pid, timeout });

    if let (Some(mut pipe), Some(input)) = (child.stdin.take(), stdin) {
        thread::spawn(move || drop(pipe.write_all(&input)));
    }
    let stdout = child.stdout.take().map(read_limited);
    let stderr = child.stderr.take().map(read_limited);

    let deadline = started + timeout;
    let (code, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.code(), false),
            Ok(None) if Instant::now() >= deadline => {
                crate::process::kill(&mut child);
                break (None, true);
            }
            Ok(None) => thread::sleep(EXIT_POLL),
            Err(error) => {
                crate::process::kill(&mut child);
                report(lua, ExecEvent::Exited { pid });
                return Err(mlua::Error::runtime(format!(
                    "waiting for {program} failed: {error}"
                )));
            }
        }
    };
    // Clean up anything the command left running in its group.
    #[cfg(unix)]
    crate::process::kill_process_group(rustix::process::Pid::from_child(&child));
    let stdout = stdout
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let stderr = stderr
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    report(lua, ExecEvent::Exited { pid });

    // Waiting on the command is not the hook's own time.
    if let Some(deadline) = lua.app_data_ref::<Deadline>()
        && let Some(at) = deadline.0.get()
    {
        deadline.0.set(Some(at + started.elapsed()));
    }

    let result = lua.create_table()?;
    result.set("code", code)?;
    result.set("stdout", lua.create_string(&stdout)?)?;
    result.set("stderr", lua.create_string(&stderr)?)?;
    result.set("timed_out", timed_out)?;
    Ok((Value::Table(result), Value::Nil))
}

struct ExecOptions {
    timeout: Duration,
    stdin: Option<Vec<u8>>,
    cwd: Option<String>,
}

impl ExecOptions {
    fn parse(options: Option<&Table>) -> mlua::Result<Self> {
        let Some(options) = options else {
            return Ok(Self {
                timeout: DEFAULT_TIMEOUT,
                stdin: None,
                cwd: None,
            });
        };
        let timeout = match options.get::<Option<u64>>("timeout_ms")? {
            None => DEFAULT_TIMEOUT,
            Some(ms) if ms > 0 && Duration::from_millis(ms) <= MAX_HOOK_BUDGET => {
                Duration::from_millis(ms)
            }
            Some(_) => {
                return Err(mlua::Error::runtime(format!(
                    "timeout_ms must be between 1 and {}",
                    MAX_HOOK_BUDGET.as_millis()
                )));
            }
        };
        Ok(Self {
            timeout,
            stdin: options
                .get::<Option<mlua::LuaString>>("stdin")?
                .map(|stdin| stdin.as_bytes().to_vec()),
            cwd: options.get("cwd")?,
        })
    }
}

fn report(lua: &Lua, event: ExecEvent) {
    if let Some(reporter) = lua.app_data_ref::<ExecReporter>() {
        (reporter.0)(event);
    }
}

fn read_limited(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buffer = [0; 8192];
        while let Ok(read) = pipe.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let room = OUTPUT_LIMIT.saturating_sub(kept.len());
            kept.extend_from_slice(&buffer[..read.min(room)]);
        }
        kept
    })
}
