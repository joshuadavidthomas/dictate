//! Plugins run in a child process so the daemon can always stop them.
//!
//! The daemon starts its own executable as `dictate plugin-host` and talks
//! JSON lines over a private loopback socket. The child's stdin carries only
//! the socket address and a token, then closes; its stdout goes to the
//! daemon's log. Nothing a plugin prints, reads, or spawns can reach the
//! protocol, and the socket is not inherited by a plugin's own child
//! processes. The in-process time budget
//! skips one runaway hook and keeps the rest; the process deadline is the
//! backstop for what that budget cannot stop, such as a blocking C call or a
//! crash. When the child misses its deadline or dies, the daemon kills it,
//! delivers the locally formatted transcript, and starts a fresh child for
//! the next dictation.

use std::hash::BuildHasher;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::net::Ipv4Addr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::process::Child;
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

/// How often the daemon checks for the host's connection.
const ACCEPT_POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Setup { setup: PluginSetup },
    Process { id: u64, raw: String },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Response {
    Hello {
        token: String,
    },
    Ready {
        plugins: Vec<PluginStatus>,
        chain_budget_ms: u64,
    },
    Processed {
        id: u64,
        text: String,
    },
}

/// The plugin-host side: read the daemon's address and token from stdin,
/// connect, receive the setup, load the plugins, report them, then answer
/// requests until the daemon closes the connection.
pub fn serve() -> io::Result<()> {
    let mut handshake = String::new();
    io::stdin().read_line(&mut handshake)?;
    let Some((address, token)) = handshake.trim().split_once(' ') else {
        return Err(io::Error::other(
            "plugin host expected `<address> <token>` on stdin",
        ));
    };

    let mut connection = TcpStream::connect(address)?;
    connection.set_nodelay(true)?;
    let mut requests = BufReader::new(connection.try_clone()?).lines();
    write_line(
        &mut connection,
        &Response::Hello {
            token: token.to_owned(),
        },
    )?;

    let Some(Ok(Request::Setup { setup })) = requests
        .next()
        .transpose()?
        .map(|line| serde_json::from_str(&line))
    else {
        return Err(io::Error::other("plugin host expected its setup first"));
    };
    let mut host = PluginHost::load(setup).map_err(io::Error::other)?;
    write_line(
        &mut connection,
        &Response::Ready {
            plugins: host.plugins().to_vec(),
            chain_budget_ms: u64::try_from(host.chain_budget().as_millis()).unwrap_or(u64::MAX),
        },
    )?;

    for line in requests {
        match serde_json::from_str::<Request>(&line?) {
            Ok(Request::Process { id, raw }) => {
                let text = host.process(&RawTranscript::new(raw));
                write_line(
                    &mut connection,
                    &Response::Processed {
                        id,
                        text: text.as_str().to_owned(),
                    },
                )?;
            }
            Ok(Request::Setup { .. }) => eprintln!("plugin host ignored a second setup"),
            Err(error) => eprintln!("plugin host ignored a malformed request: {error}"),
        }
    }
    Ok(())
}

fn write_line(connection: &mut TcpStream, message: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    connection.write_all(&line)
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
    connection: TcpStream,
    lines: mpsc::Receiver<String>,
    chain_budget: Duration,
}

impl PluginProcess {
    /// Start a plugin host. Failure to start is logged, not returned:
    /// [`Self::process`] then formats locally, and the next call tries again.
    #[must_use]
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
        let response = write_line(&mut running.connection, &request)
            .map_err(|error| format!("could not send to the plugin host: {error}"))
            .and_then(|()| {
                wait_for(&running.lines, deadline, |response| {
                    matches!(response, Response::Processed { id: done, .. } if *done == id)
                })
            });
        match response {
            Ok(Response::Processed { text, .. }) => Ok(ProcessedDictation::new(text)),
            Ok(_) => Err("plugin host sent an unexpected reply".to_owned()),
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

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
            .map_err(|error| format!("could not open the plugin host socket: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("could not open the plugin host socket: {error}"))?;
        let token = token();

        let mut command =
            (self.command)().map_err(|error| format!("could not locate dictate: {error}"))?;
        // Its own process group, so a kill also reaches anything a plugin
        // started, such as a blocked `os.execute`.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::from(io::stderr()))
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("could not start the plugin host: {error}"))?;
        // Closing stdin after the handshake line leaves plugins reading EOF.
        let handshake = child.stdin.take().map_or_else(
            || Err(io::Error::other("plugin host stdin was not created")),
            |mut stdin| writeln!(stdin, "{address} {token}"),
        );
        // Bundled plugins plus every user file get their load budget.
        let plugin_count = u32::try_from(self.fingerprint.len() + 1).unwrap_or(u32::MAX);
        let deadline = Instant::now() + LOAD_BUDGET * plugin_count + PROCESS_SLACK;
        let connected = handshake
            .map_err(|error| format!("could not send the plugin host its address: {error}"))
            .and_then(|()| accept(&listener, &mut child, &token, deadline))
            .and_then(|connection| self.start_session(connection, deadline));
        match connected {
            Ok((connection, lines, chain_budget)) => {
                self.running = Some(Running {
                    child,
                    connection,
                    lines,
                    chain_budget,
                });
                Ok(())
            }
            Err(error) => {
                kill(&mut child);
                Err(error)
            }
        }
    }

    /// Send the setup and wait for the host to report its plugins.
    fn start_session(
        &mut self,
        mut connection: TcpStream,
        deadline: Instant,
    ) -> Result<(TcpStream, mpsc::Receiver<String>, Duration), String> {
        let reader = connection
            .try_clone()
            .map_err(|error| format!("could not read from the plugin host: {error}"))?;
        let (lines_tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if lines_tx.send(line).is_err() {
                    break;
                }
            }
        });

        write_line(
            &mut connection,
            &Request::Setup {
                setup: self.setup.clone(),
            },
        )
        .map_err(|error| format!("could not send the plugin host its setup: {error}"))?;
        let timeout = deadline.saturating_duration_since(Instant::now());
        match wait_for(&lines, timeout, |response| {
            matches!(response, Response::Ready { .. })
        })? {
            Response::Ready {
                plugins,
                chain_budget_ms,
            } => {
                self.plugins = plugins;
                Ok((connection, lines, Duration::from_millis(chain_budget_ms)))
            }
            Response::Hello { .. } | Response::Processed { .. } => {
                Err("plugin host sent an unexpected reply".to_owned())
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

/// Accept the host's connection and check that it presents `token`.
fn accept(
    listener: &TcpListener,
    child: &mut Child,
    token: &str,
    deadline: Instant,
) -> Result<TcpStream, String> {
    loop {
        match listener.accept() {
            Ok((connection, _)) => {
                connection
                    .set_nonblocking(false)
                    .and_then(|()| connection.set_nodelay(true))
                    .and_then(|()| connection.set_read_timeout(Some(PROCESS_SLACK)))
                    .map_err(|error| format!("plugin host connection failed: {error}"))?;
                let mut hello = String::new();
                BufReader::new(&connection)
                    .read_line(&mut hello)
                    .map_err(|error| format!("plugin host did not say hello: {error}"))?;
                match serde_json::from_str::<Response>(&hello) {
                    Ok(Response::Hello { token: presented }) if presented == token => {
                        connection
                            .set_read_timeout(None)
                            .map_err(|error| format!("plugin host connection failed: {error}"))?;
                        return Ok(connection);
                    }
                    // Something else connected to the port; keep waiting.
                    _ => eprintln!("ignored a plugin host connection without the right token"),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("plugin host connection failed: {error}")),
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("plugin host exited before connecting: {status}"));
        }
        if Instant::now() >= deadline {
            return Err("plugin host did not connect in time".to_owned());
        }
        thread::sleep(ACCEPT_POLL);
    }
}

/// A per-host secret, so only the child this daemon started can connect.
fn token() -> String {
    let random = || std::hash::RandomState::new().hash_one((std::process::id(), SystemTime::now()));
    format!("{:016x}{:016x}", random(), random())
}

/// Wait for the first reply that `wanted` accepts.
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
                Err(error) => eprintln!("plugin host sent a malformed reply: {error}"),
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
