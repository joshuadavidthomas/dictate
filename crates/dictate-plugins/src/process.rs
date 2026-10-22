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
use std::io::Read;
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
use crate::TranscriptContext;
use crate::exec::ExecEvent;
use crate::exec::ExecReporter;
use crate::fingerprint;

/// Allowance for process startup and pipe latency on top of plugin budgets.
const PROCESS_SLACK: Duration = Duration::from_secs(1);

/// How often the daemon checks for the host's connection.
const ACCEPT_POLL: Duration = Duration::from_millis(10);
/// The longest hello line the daemon reads from a new connection.
const HELLO_LIMIT: u64 = 256;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Setup {
        setup: PluginSetup,
    },
    Process {
        id: u64,
        raw: String,
        #[serde(default)]
        context: TranscriptContext,
    },
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
    /// A plugin started a command through `dictate.exec`.
    ExecStarted {
        pid: u32,
        timeout_ms: u64,
    },
    ExecExited {
        pid: u32,
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
    let reporter = connection.try_clone()?;
    let reporter = ExecReporter(Box::new(move |event| {
        let response = match event {
            ExecEvent::Started { pid, timeout } => Response::ExecStarted {
                pid,
                timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            },
            ExecEvent::Exited { pid } => Response::ExecExited { pid },
        };
        if let Err(error) = write_line(&mut &reporter, &response) {
            eprintln!("plugin host could not report a command to the daemon: {error}");
        }
    }));
    let mut host = PluginHost::load(setup, Some(reporter)).map_err(io::Error::other)?;
    write_line(
        &mut connection,
        &Response::Ready {
            plugins: host.plugins().to_vec(),
            chain_budget_ms: u64::try_from(host.chain_budget().as_millis()).unwrap_or(u64::MAX),
        },
    )?;

    for line in requests {
        match serde_json::from_str::<Request>(&line?) {
            Ok(Request::Process { id, raw, context }) => {
                let text = host.process(&RawTranscript::new(raw), &context);
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

fn write_line(mut connection: impl Write, message: &impl Serialize) -> io::Result<()> {
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
    /// A replacement host starting on its own thread after a plugin edit.
    reloading: Option<mpsc::Receiver<Result<Started, String>>>,
    next_id: u64,
}

/// A host that finished its handshake, with the plugins it loaded.
struct Started {
    running: Running,
    plugins: Vec<PluginStatus>,
}

/// A connected plugin host. Dropping it kills the host and every command
/// its plugins are running.
struct Running {
    child: Child,
    connection: TcpStream,
    lines: mpsc::Receiver<String>,
    chain_budget: Duration,
    /// Process groups of `dictate.exec` commands still running. Each is its
    /// own group, so killing the host's group does not reach them.
    execs: Vec<u32>,
}

impl Drop for Running {
    fn drop(&mut self) {
        kill(&mut self.child);
        #[cfg(unix)]
        for &pid in &self.execs {
            if let Some(pid) = i32::try_from(pid)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
            {
                kill_process_group(pid);
            }
        }
    }
}

impl Running {
    /// Wait for the first reply that `wanted` accepts. A command a plugin
    /// starts extends the wait by its own timeout, since time spent waiting
    /// on it is not the plugin's.
    fn wait_for(
        &mut self,
        timeout: Duration,
        wanted: impl Fn(&Response) -> bool,
    ) -> Result<Response, String> {
        let mut deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) => match serde_json::from_str::<Response>(&line) {
                    Ok(Response::ExecStarted { pid, timeout_ms }) => {
                        deadline += Duration::from_millis(timeout_ms);
                        self.execs.push(pid);
                    }
                    Ok(Response::ExecExited { pid }) => {
                        self.execs.retain(|&running| running != pid);
                    }
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
            reloading: None,
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
    pub fn process(
        &mut self,
        raw: &RawTranscript,
        context: &TranscriptContext,
    ) -> ProcessedDictation {
        match self.try_process(raw, context) {
            Ok(text) => text,
            Err(error) => {
                eprintln!(
                    "plugins unavailable; delivering the locally formatted transcript: {error}"
                );
                DictationFormatter.format(raw, &self.setup.base_context())
            }
        }
    }

    /// Start a replacement host when a plugin file was added, removed, or
    /// changed. The replacement starts on its own thread, so a dictation
    /// that begins meanwhile is not held up; the current host keeps serving
    /// until the replacement is ready. Returns whether a reload started.
    pub fn reload_if_changed(&mut self) -> bool {
        self.finish_reload(false);
        if self.reloading.is_some()
            || fingerprint(self.setup.plugins_dir.as_deref()) == self.fingerprint
        {
            return false;
        }
        match self.prepare() {
            Ok(start) => {
                let (done_tx, done) = mpsc::channel();
                // If nobody receives the result, it is dropped, which kills
                // the replacement host.
                thread::spawn(move || drop(done_tx.send(start())));
                self.reloading = Some(done);
            }
            Err(error) => eprintln!("plugin host failed to restart: {error}"),
        }
        true
    }

    /// Install a finished replacement host. With `wait`, block until the
    /// replacement is ready; its own deadlines bound the wait.
    fn finish_reload(&mut self, wait: bool) {
        let Some(done) = self.reloading.as_ref() else {
            return;
        };
        let result = if wait {
            done.recv().ok()
        } else {
            match done.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => None,
            }
        };
        self.reloading = None;
        match result {
            Some(Ok(started)) => self.install(started),
            Some(Err(error)) => eprintln!("plugin host failed to restart: {error}"),
            None => eprintln!("plugin host failed to restart: the reload thread ended"),
        }
    }

    fn try_process(
        &mut self,
        raw: &RawTranscript,
        context: &TranscriptContext,
    ) -> Result<ProcessedDictation, String> {
        // A transcript is about to use the plugins, so wait for an edit to
        // take effect rather than run the old ones.
        self.finish_reload(true);
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
            context: context.clone(),
        };
        let deadline = running.chain_budget + PROCESS_SLACK;
        let response = write_line(&mut running.connection, &request)
            .map_err(|error| format!("could not send to the plugin host: {error}"))
            .and_then(|()| {
                running.wait_for(deadline, |response| {
                    matches!(response, Response::Processed { id: done, .. } if *done == id)
                })
            });
        match response {
            Ok(Response::Processed { text, .. }) => Ok(ProcessedDictation::new(text)),
            Ok(_) => Err("plugin host sent an unexpected reply".to_owned()),
            Err(error) => {
                self.running = None;
                Err(error)
            }
        }
    }

    /// Replace the host on this thread.
    fn restart(&mut self) -> Result<(), String> {
        self.reloading = None;
        self.running = None;
        self.plugins.clear();
        let started = self.prepare()?()?;
        self.install(started);
        Ok(())
    }

    fn install(&mut self, started: Started) {
        self.plugins = started.plugins;
        self.running = Some(started.running);
    }

    /// Record the plugin files being loaded and build everything a new host
    /// needs, returning the part that spawns it and waits for its handshake.
    fn prepare(&mut self) -> Result<impl FnOnce() -> Result<Started, String> + use<>, String> {
        self.fingerprint = fingerprint(self.setup.plugins_dir.as_deref());
        let command =
            (self.command)().map_err(|error| format!("could not locate dictate: {error}"))?;
        let setup = self.setup.clone();
        // Bundled plugins plus every user file get their load budget.
        let plugin_count = u32::try_from(self.fingerprint.len() + 1).unwrap_or(u32::MAX);
        let load_budget = LOAD_BUDGET * plugin_count + PROCESS_SLACK;
        Ok(move || start(command, setup, load_budget))
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        // Do not wait for a replacement host; dropping its result kills it.
        self.reloading = None;
        self.running = None;
    }
}

/// Spawn a plugin host, hand it the socket address, and wait until it has
/// connected, received `setup`, and loaded its plugins.
fn start(
    mut command: Command,
    setup: PluginSetup,
    load_budget: Duration,
) -> Result<Started, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
        .map_err(|error| format!("could not open the plugin host socket: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("could not open the plugin host socket: {error}"))?;
    let token = token();

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
    let deadline = Instant::now() + load_budget;
    let connected = handshake
        .map_err(|error| format!("could not send the plugin host its address: {error}"))
        .and_then(|()| accept(&listener, &mut child, &token, deadline));
    let (mut connection, reader) = match connected {
        Ok(connected) => connected,
        Err(error) => {
            kill(&mut child);
            return Err(error);
        }
    };

    // From here on, dropping `running` kills the host.
    let (lines_tx, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });
    write_line(&mut connection, &Request::Setup { setup })
        .map_err(|error| format!("could not send the plugin host its setup: {error}"))?;
    let mut running = Running {
        child,
        connection,
        lines,
        chain_budget: Duration::ZERO,
        execs: Vec::new(),
    };
    let timeout = deadline.saturating_duration_since(Instant::now());
    match running.wait_for(timeout, |response| {
        matches!(response, Response::Ready { .. })
    })? {
        Response::Ready {
            plugins,
            chain_budget_ms,
        } => {
            running.chain_budget = Duration::from_millis(chain_budget_ms);
            Ok(Started { running, plugins })
        }
        Response::Hello { .. }
        | Response::Processed { .. }
        | Response::ExecStarted { .. }
        | Response::ExecExited { .. } => Err("plugin host sent an unexpected reply".to_owned()),
    }
}

/// Accept the host's connection and check that it presents `token`. Returns
/// the connection and a reader that keeps anything the host sent after its
/// hello.
fn accept(
    listener: &TcpListener,
    child: &mut Child,
    token: &str,
    deadline: Instant,
) -> Result<(TcpStream, BufReader<TcpStream>), String> {
    loop {
        match listener.accept() {
            Ok((connection, _)) => match hello(connection, token) {
                Ok(connected) => return Ok(connected),
                // Something else connected to the port; keep waiting.
                Err(error) => eprintln!("ignored a plugin host connection: {error}"),
            },
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

/// Read one bounded hello line from a new connection and check its token.
fn hello(connection: TcpStream, token: &str) -> Result<(TcpStream, BufReader<TcpStream>), String> {
    connection
        .set_nonblocking(false)
        .and_then(|()| connection.set_nodelay(true))
        .and_then(|()| connection.set_read_timeout(Some(PROCESS_SLACK)))
        .map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(connection.try_clone().map_err(|error| error.to_string())?);
    let mut line = String::new();
    reader
        .by_ref()
        .take(HELLO_LIMIT)
        .read_line(&mut line)
        .map_err(|error| format!("no hello: {error}"))?;
    match serde_json::from_str::<Response>(&line) {
        Ok(Response::Hello { token: presented }) if presented == token => {
            connection
                .set_read_timeout(None)
                .map_err(|error| error.to_string())?;
            Ok((connection, reader))
        }
        _ => Err("it did not present the plugin host's token".to_owned()),
    }
}

/// A per-host secret, so only the child this daemon started can connect.
fn token() -> String {
    let random = || std::hash::RandomState::new().hash_one((std::process::id(), SystemTime::now()));
    format!("{:016x}{:016x}", random(), random())
}

/// Kill the plugin host and, on Unix, every process in its group. On
/// Windows only the host itself is killed.
pub(crate) fn kill(child: &mut Child) {
    #[cfg(unix)]
    kill_process_group(rustix::process::Pid::from_child(child));
    drop(child.kill());
    drop(child.wait());
}

/// Kill every process in group `pid`; a group that is already gone is fine.
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: rustix::process::Pid) {
    if let Err(error) = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL)
        && error != rustix::io::Errno::SRCH
    {
        eprintln!(
            "could not kill process group {}: {error}",
            pid.as_raw_nonzero()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn accept_skips_strangers_and_keeps_bytes_after_the_hello() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("listener should bind");
        listener
            .set_nonblocking(true)
            .expect("listener should be non-blocking");
        let address = listener
            .local_addr()
            .expect("listener should have an address");
        let mut child = Command::new("sleep")
            .arg("10")
            .spawn()
            .expect("sleep should start");

        let connections = thread::spawn(move || {
            // A stranger that never speaks, one that sends garbage without a
            // newline, then the host, which sends more right after its hello.
            let silent = TcpStream::connect(address).expect("silent should connect");
            let mut garbage = TcpStream::connect(address).expect("garbage should connect");
            garbage
                .write_all(&[b'x'; 1024])
                .expect("garbage should send");
            let mut host = TcpStream::connect(address).expect("host should connect");
            host.write_all(b"{\"type\":\"hello\",\"token\":\"secret\"}\nafter hello\n")
                .expect("host should send");
            (silent, garbage, host)
        });

        let result = accept(
            &listener,
            &mut child,
            "secret",
            Instant::now() + Duration::from_secs(5),
        );
        let _connections = connections.join().expect("connections should be made");
        kill(&mut child);

        let (_connection, mut reader) = result.expect("the host should be accepted");
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("the reader should keep later bytes");
        assert_eq!(line, "after hello\n");
    }
}
