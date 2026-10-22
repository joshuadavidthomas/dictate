//! Plugins run in a child process so the daemon can always stop them.
//!
//! The daemon starts its own executable as `dictate plugin-host` and talks
//! JSON lines over a private loopback socket. The child's stdin carries only
//! the socket address and a token, then closes; its stdout goes to the
//! daemon's log. Nothing a plugin prints, reads, or spawns can reach the
//! protocol, and the socket is not inherited by a plugin's own child
//! processes.
//!
//! The daemon side, [`PluginProcess`], never waits on the host. It sends
//! transcripts and events, and the host's replies and core calls come back
//! as [`HostMessage`]s that the owner's thread hands to
//! [`PluginProcess::receive`]. The in-process time budget skips one runaway
//! hook and keeps the rest; the process deadline is the backstop for what
//! that budget cannot stop, such as a blocking C call or a crash. When the
//! child misses a deadline or dies, the daemon kills it, formats the
//! transcript locally, and starts a fresh child for the next transcript.

use std::hash::BuildHasher;
use std::io;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::net::Ipv4Addr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::mpsc;
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
use crate::Message;
use crate::PluginHost;
use crate::PluginSetup;
use crate::PluginStatus;
use crate::TranscriptContext;
use crate::core::CoreCall;
use crate::core::CoreResult;
use crate::core::PluginEvent;
use crate::exec::ExecEvent;
use crate::fingerprint;

/// Allowance for process startup and pipe latency on top of plugin budgets.
const PROCESS_SLACK: Duration = Duration::from_secs(1);

/// How often the daemon checks for the host's connection.
const ACCEPT_POLL: Duration = Duration::from_millis(10);
/// The longest hello line the daemon reads from a new connection.
const HELLO_LIMIT: u64 = 256;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Request {
    Setup {
        setup: PluginSetup,
    },
    Process {
        id: u64,
        raw: String,
        #[serde(default)]
        context: TranscriptContext,
    },
    Event {
        event: PluginEvent,
    },
    Reply {
        call: u64,
        result: CoreResult,
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
    /// Plugin code called into the core; the daemon answers with a reply.
    Call {
        call: u64,
        op: CoreCall,
    },
}

/// The plugin-host side: read the daemon's address and token from stdin,
/// connect, receive the setup, load the plugins, report them, then serve
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

    // One writer, so lines from command threads and the Lua thread never
    // interleave.
    let (responses, outgoing) = mpsc::channel::<Response>();
    let mut writer = connection;
    thread::spawn(move || {
        for response in outgoing {
            if let Err(error) = write_line(&mut writer, &response) {
                eprintln!("plugin host could not write to the daemon: {error}");
                break;
            }
        }
    });

    let reporter = responses.clone();
    let reporter: crate::exec::Reporter = Arc::new(move |event| {
        let response = match event {
            ExecEvent::Started { pid, timeout } => Response::ExecStarted {
                pid,
                timeout_ms: millis(timeout),
            },
            ExecEvent::Exited { pid } => Response::ExecExited { pid },
        };
        drop(reporter.send(response));
    });
    let mut host = PluginHost::load(setup, Some(reporter)).map_err(io::Error::other)?;
    drop(responses.send(Response::Ready {
        plugins: host.plugins().to_vec(),
        chain_budget_ms: millis(host.chain_budget()),
    }));

    let inbox = host.sender();
    thread::spawn(move || {
        for line in requests {
            let Ok(line) = line else { break };
            match serde_json::from_str::<Request>(&line) {
                Ok(request) => {
                    if inbox.send(Message::Request(request)).is_err() {
                        return;
                    }
                }
                Err(error) => eprintln!("plugin host ignored a malformed request: {error}"),
            }
        }
        drop(inbox.send(Message::Closed));
    });

    loop {
        match host.next_message() {
            Message::Request(Request::Process { id, raw, context }) => {
                host.start_chain(id, &RawTranscript::new(raw), &context);
            }
            Message::Request(Request::Event { event }) => host.dispatch(&event),
            Message::Request(Request::Reply { call, result }) => host.finish_call(call, result),
            Message::Request(Request::Setup { .. }) => {
                eprintln!("plugin host ignored a second setup");
            }
            Message::Exec { task, outcome } => host.finish_exec(task, outcome),
            Message::Closed => return Ok(()),
        }
        for (call, op) in host.take_calls() {
            drop(responses.send(Response::Call { call, op }));
        }
        for (id, text) in host.take_finished() {
            drop(responses.send(Response::Processed {
                id,
                text: text.as_str().to_owned(),
            }));
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn write_line(mut connection: impl Write, message: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    connection.write_all(&line)
}

/// Builds the command that starts a plugin host.
pub type HostCommand = Box<dyn Fn() -> io::Result<Command> + Send>;

/// Delivers [`HostMessage`]s to the thread that owns the [`PluginProcess`].
/// Called from the host's reader thread and from reload threads.
pub type Notify = Arc<dyn Fn(HostMessage) + Send + Sync>;

/// Something from a plugin host, to hand to [`PluginProcess::receive`] on
/// the owner's thread.
pub struct HostMessage(Incoming);

enum Incoming {
    Line { generation: u64, line: String },
    Disconnected { generation: u64 },
    Started(Result<Started, String>),
}

/// One plugin host's lifetime: a restart gets a new id.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HostId(u64);

/// A core call from plugin code, to answer with [`PluginProcess::reply`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CallId {
    generation: u64,
    call: u64,
}

impl CallId {
    /// The host that made the call.
    #[must_use]
    pub fn host(self) -> HostId {
        HostId(self.generation)
    }
}

/// What [`PluginProcess`] reports back to its owner.
#[derive(Debug)]
pub enum HostEvent {
    /// The plugin chain finished transcript `id`. When the host was missing,
    /// overran, or died, the text was formatted locally instead.
    Processed { id: u64, text: ProcessedDictation },
    /// Plugin code called into the core.
    Call { call: CallId, op: CoreCall },
    /// A host stopped. Calls it made need no reply, and anything it owned in
    /// the core is the core's again.
    HostStopped(HostId),
}

/// The daemon side: a supervised plugin-host child.
pub struct PluginProcess {
    command: HostCommand,
    setup: PluginSetup,
    notify: Notify,
    running: Option<Running>,
    plugins: Vec<PluginStatus>,
    fingerprint: Vec<(PathBuf, Option<SystemTime>, u64)>,
    /// A host starting on its own thread; its result arrives as a message.
    starting: bool,
    next_generation: u64,
    /// Transcripts sent to the host (with a deadline) or waiting for a host
    /// to start (without one).
    pending: Vec<Pending>,
}

struct Pending {
    id: u64,
    raw: RawTranscript,
    context: TranscriptContext,
    deadline: Option<Instant>,
}

/// A host that finished its handshake, with the plugins it loaded.
struct Started {
    running: Running,
    plugins: Vec<PluginStatus>,
}

/// A connected plugin host. Dropping it kills the host and every command
/// its plugins are running.
struct Running {
    generation: u64,
    child: Child,
    connection: TcpStream,
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

impl PluginProcess {
    /// Start a plugin host on its own thread; [`HostMessage`]s arrive through
    /// `notify`. Transcripts sent before it is ready wait for it.
    #[must_use]
    pub fn new(command: HostCommand, setup: PluginSetup, notify: Notify) -> Self {
        let mut process = Self {
            command,
            setup,
            notify,
            running: None,
            plugins: Vec::new(),
            fingerprint: Vec::new(),
            starting: false,
            next_generation: 1,
            pending: Vec::new(),
        };
        process.start();
        process
    }

    /// The plugins the running host loaded, with their errors.
    #[must_use]
    fn plugins(&self) -> &[PluginStatus] {
        &self.plugins
    }

    #[must_use]
    pub(crate) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Whether a host is starting.
    #[must_use]
    pub(crate) fn is_starting(&self) -> bool {
        self.starting
    }

    /// Run the plugin chain on transcript `id`; the result arrives as
    /// [`HostEvent::Processed`], possibly in the returned events.
    pub fn transcript(
        &mut self,
        id: u64,
        raw: &RawTranscript,
        context: &TranscriptContext,
    ) -> Vec<HostEvent> {
        self.pending.push(Pending {
            id,
            raw: raw.clone(),
            context: context.clone(),
            deadline: None,
        });
        if self.running.is_none() && !self.starting {
            self.start();
        }
        if self.starting {
            // Wait for the edit or restart to take effect.
            return Vec::new();
        }
        self.send_pending()
    }

    /// Send an event to the running host's handlers. Without a host, the
    /// event is dropped.
    pub fn event(&mut self, event: PluginEvent) -> Vec<HostEvent> {
        self.send(&Request::Event { event })
    }

    /// Answer a core call. A reply to a host that has since stopped is
    /// dropped.
    pub fn reply(&mut self, call: CallId, result: CoreResult) -> Vec<HostEvent> {
        if self
            .running
            .as_ref()
            .is_some_and(|running| running.generation == call.generation)
        {
            return self.send(&Request::Reply {
                call: call.call,
                result,
            });
        }
        Vec::new()
    }

    /// Handle a message from a host.
    pub fn receive(&mut self, message: HostMessage) -> Vec<HostEvent> {
        match message.0 {
            Incoming::Started(result) => {
                self.starting = false;
                match result {
                    Ok(started) => {
                        let mut events = self.stop_running();
                        self.plugins = started.plugins;
                        self.running = Some(started.running);
                        events.extend(self.send_pending());
                        events
                    }
                    Err(error) => {
                        eprintln!("plugin host failed to start: {error}");
                        if self.running.is_some() {
                            // The old host keeps serving until a later edit.
                            self.send_pending()
                        } else {
                            self.fail_pending()
                        }
                    }
                }
            }
            Incoming::Disconnected { generation } if self.is_current(generation) => {
                self.stop("plugin host exited")
            }
            Incoming::Line { generation, line } if self.is_current(generation) => {
                self.receive_line(&line)
            }
            Incoming::Disconnected { .. } | Incoming::Line { .. } => Vec::new(),
        }
    }

    /// When [`Self::check_deadlines`] next needs to run.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending
            .iter()
            .filter_map(|pending| pending.deadline)
            .min()
    }

    /// Kill a host that missed a transcript's deadline.
    pub fn check_deadlines(&mut self) -> Vec<HostEvent> {
        let now = Instant::now();
        if self
            .pending
            .iter()
            .any(|pending| pending.deadline.is_some_and(|deadline| deadline <= now))
        {
            return self.stop("plugin host did not answer in time");
        }
        Vec::new()
    }

    /// Start a replacement host when a plugin file was added, removed, or
    /// changed. The current host keeps serving events until the replacement
    /// is ready; transcripts wait for it. Returns whether a restart started.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.starting || fingerprint(self.setup.plugins_dir.as_deref()) == self.fingerprint {
            return false;
        }
        self.start();
        true
    }

    fn is_current(&self, generation: u64) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| running.generation == generation)
    }

    fn receive_line(&mut self, line: &str) -> Vec<HostEvent> {
        let response = match serde_json::from_str::<Response>(line) {
            Ok(response) => response,
            Err(error) => {
                eprintln!("plugin host sent a malformed message: {error}");
                return Vec::new();
            }
        };
        let Some(running) = self.running.as_mut() else {
            return Vec::new();
        };
        match response {
            Response::Processed { id, text } => {
                let Some(index) = self
                    .pending
                    .iter()
                    .position(|pending| pending.id == id && pending.deadline.is_some())
                else {
                    return Vec::new();
                };
                self.pending.remove(index);
                vec![HostEvent::Processed {
                    id,
                    text: ProcessedDictation::new(text),
                }]
            }
            Response::ExecStarted { pid, timeout_ms } => {
                // Waiting on a command is not the plugin's time.
                running.execs.push(pid);
                for deadline in self
                    .pending
                    .iter_mut()
                    .filter_map(|pending| pending.deadline.as_mut())
                {
                    *deadline += Duration::from_millis(timeout_ms);
                }
                Vec::new()
            }
            Response::ExecExited { pid } => {
                running.execs.retain(|&running| running != pid);
                Vec::new()
            }
            Response::Call { call, op } => vec![HostEvent::Call {
                call: CallId {
                    generation: running.generation,
                    call,
                },
                op,
            }],
            Response::Hello { .. } | Response::Ready { .. } => Vec::new(),
        }
    }

    /// Send every waiting transcript to the running host.
    fn send_pending(&mut self) -> Vec<HostEvent> {
        let Some(running) = self.running.as_ref() else {
            return Vec::new();
        };
        let deadline = Instant::now() + running.chain_budget + PROCESS_SLACK;
        let mut requests = Vec::new();
        for pending in self
            .pending
            .iter_mut()
            .filter(|pending| pending.deadline.is_none())
        {
            pending.deadline = Some(deadline);
            requests.push(Request::Process {
                id: pending.id,
                raw: pending.raw.as_str().to_owned(),
                context: pending.context.clone(),
            });
        }
        let mut events = Vec::new();
        for request in requests {
            events.extend(self.send(&request));
        }
        events
    }

    fn send(&mut self, request: &Request) -> Vec<HostEvent> {
        let Some(running) = self.running.as_mut() else {
            return Vec::new();
        };
        match write_line(&mut running.connection, request) {
            Ok(()) => Vec::new(),
            Err(error) => self.stop(&format!("could not send to the plugin host: {error}")),
        }
    }

    /// Stop the running host and format everything it was working on
    /// locally.
    fn stop(&mut self, why: &str) -> Vec<HostEvent> {
        eprintln!("plugins unavailable; formatting transcripts locally: {why}");
        let mut events = self.stop_running();
        events.extend(self.fail_pending());
        events
    }

    fn stop_running(&mut self) -> Vec<HostEvent> {
        let Some(running) = self.running.take() else {
            return Vec::new();
        };
        let host = HostId(running.generation);
        drop(running);
        // Transcripts the old host had are sent again to the next one, or
        // formatted locally if there is none.
        for pending in &mut self.pending {
            pending.deadline = None;
        }
        vec![HostEvent::HostStopped(host)]
    }

    fn fail_pending(&mut self) -> Vec<HostEvent> {
        let context = self.setup.base_context();
        self.pending
            .drain(..)
            .map(|pending| HostEvent::Processed {
                id: pending.id,
                text: DictationFormatter.format(&pending.raw, &context),
            })
            .collect()
    }

    /// Start a host on its own thread.
    fn start(&mut self) {
        self.fingerprint = fingerprint(self.setup.plugins_dir.as_deref());
        let command = match (self.command)() {
            Ok(command) => command,
            Err(error) => {
                eprintln!("plugin host failed to start: could not locate dictate: {error}");
                return;
            }
        };
        let setup = self.setup.clone();
        // Bundled plugins plus every user file get their load budget.
        let plugin_count =
            u32::try_from(self.fingerprint.len() + crate::BUNDLED.len()).unwrap_or(u32::MAX);
        let load_budget = LOAD_BUDGET * plugin_count + PROCESS_SLACK;
        let generation = self.next_generation;
        self.next_generation += 1;
        let notify = Arc::clone(&self.notify);
        self.starting = true;
        thread::spawn(move || {
            let started = start(command, setup, load_budget, generation, &notify);
            notify(HostMessage(Incoming::Started(started)));
        });
    }
}

/// Spawn a plugin host, hand it the socket address, and wait until it has
/// connected, received `setup`, and loaded its plugins. Its later messages
/// go to `notify`.
fn start(
    mut command: Command,
    setup: PluginSetup,
    load_budget: Duration,
    generation: u64,
    notify: &Notify,
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
    let mut deadline = Instant::now() + load_budget;
    let connected = handshake
        .map_err(|error| format!("could not send the plugin host its address: {error}"))
        .and_then(|()| accept(&listener, &mut child, &token, deadline));
    let (connection, mut reader) = match connected {
        Ok(connected) => connected,
        Err(error) => {
            kill(&mut child);
            return Err(error);
        }
    };

    // From here on, dropping `running` kills the host.
    let mut running = Running {
        generation,
        child,
        connection,
        chain_budget: Duration::ZERO,
        execs: Vec::new(),
    };
    write_line(&mut running.connection, &Request::Setup { setup })
        .map_err(|error| format!("could not send the plugin host its setup: {error}"))?;

    let plugins = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("plugin host did not load its plugins in time".to_owned());
        }
        reader
            .get_ref()
            .set_read_timeout(Some(remaining))
            .map_err(|error| error.to_string())?;
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return Err("plugin host exited while loading plugins".to_owned()),
            Ok(_) => {}
            Err(error) => return Err(format!("plugin host did not load its plugins: {error}")),
        }
        match serde_json::from_str::<Response>(&line) {
            Ok(Response::Ready {
                plugins,
                chain_budget_ms,
            }) => {
                running.chain_budget = Duration::from_millis(chain_budget_ms);
                break plugins;
            }
            Ok(Response::ExecStarted { pid, timeout_ms }) => {
                deadline += Duration::from_millis(timeout_ms);
                running.execs.push(pid);
            }
            Ok(Response::ExecExited { pid }) => running.execs.retain(|&running| running != pid),
            Ok(_) => return Err("plugin host sent an unexpected reply while loading".to_owned()),
            Err(error) => eprintln!("plugin host sent a malformed message: {error}"),
        }
    };
    reader
        .get_ref()
        .set_read_timeout(None)
        .map_err(|error| error.to_string())?;

    let notify = Arc::clone(notify);
    thread::spawn(move || {
        for line in reader.lines() {
            let Ok(line) = line else { break };
            notify(HostMessage(Incoming::Line { generation, line }));
        }
        notify(HostMessage(Incoming::Disconnected { generation }));
    });
    Ok(Started { running, plugins })
}

/// A [`PluginProcess`] driven to completion on the caller's thread, for
/// command-line use. Core calls are answered with an error: there is no
/// daemon to serve them.
pub struct BlockingPlugins {
    process: PluginProcess,
    inbox: mpsc::Receiver<HostMessage>,
    next_id: u64,
}

impl BlockingPlugins {
    /// Start a plugin host and wait until it has loaded, or failed to.
    #[must_use]
    pub fn new(command: HostCommand, setup: PluginSetup) -> Self {
        let (inbox_tx, inbox) = mpsc::channel();
        let process = PluginProcess::new(
            command,
            setup,
            Arc::new(move |message| drop(inbox_tx.send(message))),
        );
        let mut plugins = Self {
            process,
            inbox,
            next_id: 1,
        };
        plugins.wait_for_start();
        plugins
    }

    #[must_use]
    pub fn plugins(&self) -> &[PluginStatus] {
        self.process.plugins()
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.process.is_running()
    }

    /// Run the plugin chain; if the host is missing, overruns, or dies, the
    /// transcript is formatted locally instead.
    pub fn process(
        &mut self,
        raw: &RawTranscript,
        context: &TranscriptContext,
    ) -> ProcessedDictation {
        let id = self.next_id;
        self.next_id += 1;
        let mut events = self.process.transcript(id, raw, context);
        loop {
            for event in std::mem::take(&mut events) {
                match event {
                    HostEvent::Processed { id: done, text } if done == id => return text,
                    HostEvent::Call { call, .. } => events.extend(
                        self.process
                            .reply(call, Err("core calls need the running daemon".to_owned())),
                    ),
                    HostEvent::Processed { .. } | HostEvent::HostStopped(_) => {}
                }
            }
            if !events.is_empty() {
                continue;
            }
            let wait = self
                .process
                .next_deadline()
                .map_or(Duration::from_mins(1), |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                });
            events = match self.inbox.recv_timeout(wait) {
                Ok(message) => self.process.receive(message),
                Err(_) => self.process.check_deadlines(),
            };
        }
    }

    /// Start a replacement host after plugin edits, without waiting for
    /// it; the next transcript waits for it instead.
    #[cfg(test)]
    pub(crate) fn reload_if_changed(&mut self) -> bool {
        self.process.reload_if_changed()
    }

    fn wait_for_start(&mut self) {
        while self.process.is_starting() {
            let Ok(message) = self.inbox.recv() else {
                return;
            };
            drop(self.process.receive(message));
        }
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
