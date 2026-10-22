//! `dictate.exec`: run a command with a timeout the core enforces.
//!
//! The calling coroutine parks while the command runs on its own thread,
//! so waiting is not the hook's time and other plugin work goes on. The
//! daemon is told about the command so it can extend its own deadline and
//! kill the command if it kills the plugin host.

use std::io::Read;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use mlua::IntoLuaMulti;
use mlua::Lua;
use mlua::LuaSerdeExt;
use mlua::MultiValue;
use mlua::Value;

use crate::api::ExecOptions;
use crate::api::ExecResult;
use crate::hook_budget;
use crate::json_to_lua;
use crate::runtime::Call;
use crate::runtime::park;

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

/// Receives [`ExecEvent`]s from whichever thread runs the command; absent
/// when plugins run in-process (tests).
pub(crate) type Reporter = Arc<dyn Fn(ExecEvent) + Send + Sync>;

struct ExecReporter(Reporter);

pub(crate) fn set_reporter(lua: &Lua, reporter: Reporter) {
    lua.set_app_data(ExecReporter(reporter));
}

pub(crate) fn reporter(lua: &Lua) -> Option<Reporter> {
    lua.app_data_ref::<ExecReporter>()
        .map(|reporter| Arc::clone(&reporter.0))
}

/// A command a plugin asked to run.
pub(crate) struct ExecRequest {
    program: String,
    args: Vec<String>,
    timeout: Duration,
    stdin: Option<String>,
    cwd: Option<String>,
}

/// `dictate.exec({ program, args... }, { timeout_ms, stdin, cwd })`.
///
/// Parks the calling coroutine until the command ends. Returns
/// `{ code, stdout, stderr, timed_out }`, where `code` is nil when the
/// command was killed, or `nil, message` when it could not run.
pub(crate) async fn exec(
    lua: Lua,
    (command_line, options): (Vec<String>, Option<Value>),
) -> mlua::Result<MultiValue> {
    let Some((program, args)) = command_line.split_first() else {
        return Err(mlua::Error::runtime("dictate.exec needs a program name"));
    };
    let ExecOptions {
        timeout_ms,
        stdin,
        cwd,
    } = match options {
        Some(options) => lua.from_value(options)?,
        None => ExecOptions::default(),
    };
    let request = ExecRequest {
        program: program.clone(),
        args: args.to_vec(),
        timeout: match timeout_ms {
            Some(_) => hook_budget(timeout_ms)?,
            None => DEFAULT_TIMEOUT,
        },
        stdin,
        cwd,
    };
    park(&lua, "dictate.exec", Call::Exec(request)).await
}

/// What `dictate.exec` returns for `outcome`.
pub(crate) fn into_lua(lua: &Lua, outcome: Result<ExecResult, String>) -> mlua::Result<MultiValue> {
    match outcome {
        Ok(result) => (lua.to_value_with(&result, json_to_lua())?, Value::Nil).into_lua_multi(lua),
        Err(message) => (Value::Nil, message).into_lua_multi(lua),
    }
}

/// Run `request` to completion on this thread.
pub(crate) fn run(
    request: &ExecRequest,
    report: Option<&(dyn Fn(ExecEvent) + Send + Sync)>,
) -> Result<ExecResult, String> {
    let report = |event| {
        if let Some(report) = report {
            report(event);
        }
    };
    let program = &request.program;
    let mut command = Command::new(program);
    command
        .args(&request.args)
        .stdin(if request.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    // Its own process group, so a timeout also reaches what it started.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);

    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run {program}: {error}"))?;
    let pid = child.id();
    report(ExecEvent::Started {
        pid,
        timeout: request.timeout,
    });

    if let (Some(mut pipe), Some(input)) = (child.stdin.take(), request.stdin.clone()) {
        thread::spawn(move || drop(pipe.write_all(input.as_bytes())));
    }
    let stdout = child.stdout.take().map(read_limited);
    let stderr = child.stderr.take().map(read_limited);

    let deadline = started + request.timeout;
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
                report(ExecEvent::Exited { pid });
                return Err(format!("waiting for {program} failed: {error}"));
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
    report(ExecEvent::Exited { pid });

    Ok(ExecResult {
        code,
        stdout,
        stderr,
        timed_out,
    })
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
