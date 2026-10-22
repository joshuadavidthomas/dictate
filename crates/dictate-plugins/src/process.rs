//! Plugins run in a child process so the daemon can always stop them.
//!
//! The daemon starts its own executable as `dictate plugin-host` and talks
//! JSON lines over the child's stdin and stdout. The in-process time budget
//! skips one runaway hook and keeps the rest; the process deadline is the
//! backstop for what that budget cannot stop, such as a blocking C call or a
//! crash. When the child misses its deadline or dies, the daemon kills it,
//! delivers the locally formatted transcript, and starts a fresh child for
//! the next dictation.

use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::process::Child;
use std::process::ChildStdin;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use dictate_speech::DictationFormatter;
use dictate_speech::ProcessedDictation;
use dictate_speech::RawTranscript;
use serde::Deserialize;
use serde::Serialize;

use crate::LOAD_BUDGET;
use crate::PluginHost;
use crate::PluginSetup;
use crate::PluginStatus;
use crate::fingerprint;

/// Allowance for process startup and pipe latency on top of plugin budgets.
const PROCESS_SLACK: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Process { id: u64, raw: String },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response {
    Ready {
        plugins: Vec<PluginStatus>,
        chain_budget_ms: u64,
    },
    Processed {
        id: u64,
        text: String,
    },
}

/// The plugin-host side: load the plugins, report them, then answer
/// requests from stdin until it closes.
pub fn serve(setup: PluginSetup) -> io::Result<()> {
    let mut host = PluginHost::load(setup).map_err(io::Error::other)?;
    send(&Response::Ready {
        plugins: host.plugins().to_vec(),
        chain_budget_ms: u64::try_from(host.chain_budget().as_millis()).unwrap_or(u64::MAX),
    })?;

    for line in io::stdin().lock().lines() {
        let line = line?;
        match serde_json::from_str::<Request>(&line) {
            Ok(Request::Process { id, raw }) => {
                let text = host.process(&RawTranscript::new(raw));
                send(&Response::Processed {
                    id,
                    text: text.as_str().to_owned(),
                })?;
            }
            Err(error) => eprintln!("plugin host ignored a malformed request: {error}"),
        }
    }
    Ok(())
}

fn send(response: &Response) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, response)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

/// Builds the command that starts a plugin host.
pub type HostCommand = Box<dyn Fn() -> io::Result<Command> + Send>;

/// The daemon side: a supervised plugin-host child.
pub struct PluginProcess {
    command: HostCommand,
    setup: PluginSetup,
    running: Option<Running>,
    plugins: Vec<PluginStatus>,
    fingerprint: Vec<(std::path::PathBuf, Option<SystemTime>, u64)>,
    next_id: u64,
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    chain_budget: Duration,
}

impl PluginProcess {
    /// Start a plugin host. Failure to start is logged, not returned:
    /// [`Self::process`] then formats locally, and the next call tries again.
    pub fn new(command: HostCommand, setup: PluginSetup) -> Self {
        let mut process = Self {
            command,
            setup,
            running: None,
            plugins: Vec::new(),
            fingerprint: Vec::new(),
            next_id: 1,
        };
        if let Err(error) = process.restart() {
            eprintln!("plugin host failed to start: {error}");
        }
        process
    }

    /// The plugins the running host loaded, with their errors.
    #[must_use]
    pub fn plugins(&self) -> &[PluginStatus] {
        &self.plugins
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Run the plugin chain. If the host is missing, overruns, or dies, the
    /// transcript is formatted locally with the configured context instead.
    pub fn process(&mut self, raw: &RawTranscript) -> ProcessedDictation {
        match self.try_process(raw) {
            Ok(text) => text,
            Err(error) => {
                eprintln!(
                    "plugins unavailable; delivering the locally formatted transcript: {error}"
                );
                DictationFormatter.format(raw, &self.setup.base_context())
            }
        }
    }

    /// Start a fresh host when a plugin file was added, removed, or changed.
    /// Returns whether a restart happened.
    pub fn reload_if_changed(&mut self) -> bool {
        if fingerprint(self.setup.plugins_dir.as_deref()) == self.fingerprint {
            return false;
        }
        if let Err(error) = self.restart() {
            eprintln!("plugin host failed to restart: {error}");
        }
        true
    }

    fn try_process(&mut self, raw: &RawTranscript) -> Result<ProcessedDictation, String> {
        if self.running.is_none() {
            self.restart()?;
        }
        let id = self.next_id;
        self.next_id += 1;
        let Some(running) = self.running.as_mut() else {
            return Err("plugin host is not running".to_owned());
        };

        let request = Request::Process {
            id,
            raw: raw.as_str().to_owned(),
        };
        let deadline = running.chain_budget + PROCESS_SLACK;
        let response = write_request(&mut running.stdin, &request)
            .map_err(|error| format!("could not send to the plugin host: {error}"))
            .and_then(|()| {
                wait_for(&running.lines, deadline, |response| {
                    matches!(response, Response::Processed { id: done, .. } if *done == id)
                })
            });
        match response {
            Ok(Response::Processed { text, .. }) => Ok(ProcessedDictation::new(text)),
            Ok(Response::Ready { .. }) => Err("plugin host sent an unexpected reply".to_owned()),
            Err(error) => {
                self.stop();
                Err(error)
            }
        }
    }

    fn restart(&mut self) -> Result<(), String> {
        self.stop();
        self.plugins.clear();
        self.fingerprint = fingerprint(self.setup.plugins_dir.as_deref());

        let mut command =
            (self.command)().map_err(|error| format!("could not locate dictate: {error}"))?;
        // Its own process group, so a kill also reaches anything a plugin
        // started, such as a blocked `os.execute`.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("could not start the plugin host: {error}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            kill(&mut child);
            return Err("plugin host pipes were not created".to_owned());
        };

        let (lines_tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines_tx.send(line).is_err() {
                    break;
                }
            }
        });

        // Bundled plugins plus every user file get their load budget.
        let plugin_count = u32::try_from(self.fingerprint.len() + 1).unwrap_or(u32::MAX);
        let load_deadline = LOAD_BUDGET * plugin_count + PROCESS_SLACK;
        match wait_for(&lines, load_deadline, |response| {
            matches!(response, Response::Ready { .. })
        }) {
            Ok(Response::Ready {
                plugins,
                chain_budget_ms,
            }) => {
                self.plugins = plugins;
                self.running = Some(Running {
                    child,
                    stdin,
                    lines,
                    chain_budget: Duration::from_millis(chain_budget_ms),
                });
                Ok(())
            }
            Ok(Response::Processed { .. }) => {
                kill(&mut child);
                Err("plugin host sent an unexpected reply".to_owned())
            }
            Err(error) => {
                kill(&mut child);
                Err(error)
            }
        }
    }

    fn stop(&mut self) {
        if let Some(mut running) = self.running.take() {
            kill(&mut running.child);
        }
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn write_request(stdin: &mut ChildStdin, request: &Request) -> io::Result<()> {
    let mut line = serde_json::to_vec(request)?;
    line.push(b'\n');
    stdin.write_all(&line)?;
    stdin.flush()
}

/// Wait for the first reply that `wanted` accepts. Lines that are not
/// replies (output a plugin wrote straight to stdout) are passed to the log.
fn wait_for(
    lines: &mpsc::Receiver<String>,
    timeout: Duration,
    wanted: impl Fn(&Response) -> bool,
) -> Result<Response, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(remaining) {
            Ok(line) => match serde_json::from_str::<Response>(&line) {
                Ok(response) if wanted(&response) => return Ok(response),
                Ok(_) => {}
                Err(_) => eprintln!("plugin output: {line}"),
            },
            Err(RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "plugin host did not answer within {} ms",
                    timeout.as_millis()
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err("plugin host exited".to_owned());
            }
        }
    }
}

/// Kill the plugin host and, on Unix, every process in its group. On
/// Windows only the host itself is killed.
fn kill(child: &mut Child) {
    #[cfg(unix)]
    if let Err(error) = rustix::process::kill_process_group(
        rustix::process::Pid::from_child(child),
        rustix::process::Signal::KILL,
    ) && error != rustix::io::Errno::SRCH
    {
        eprintln!("could not kill the plugin host's process group: {error}");
    }
    drop(child.kill());
    drop(child.wait());
}
