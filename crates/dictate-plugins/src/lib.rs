//! Lua plugin host for the dictation pipeline.
//!
//! Plugins are trusted Lua 5.4 files that run in the daemon's process, on
//! the thread that owns [`Plugins`]. As in `OpenCode` v2, there are no time
//! limits and no fallback flow: dictation itself is the bundled
//! `dictation.lua`.
//!
//! Plugins extend Dictate in two ways:
//!
//! - **Registries** (`dictate.dictionary`, `dictate.replacements`) are
//!   rebuilt, never mutated: each rebuild starts from the configured value
//!   and runs every registered `transform` once, in plugin order. Each
//!   transform gets a fresh copy. A transform that errors, or returns
//!   something that is not the registry's type, disables its plugin, and
//!   the registries are rebuilt without it.
//! - **Handlers** (`dictate.on(event, fn)`): `transcript` hooks form an
//!   ordered text-in, text-out chain; `hotkey`, `record`, and `session`
//!   handlers run on daemon events.
//!
//! Everything a plugin registers or opens (transforms, handlers, waiting
//! calls, commands, recording sessions) belongs to it and goes away when
//! it is unloaded. When files change, plugins before the first changed one
//! stay loaded; that one and every later one are unloaded in reverse and
//! loaded again in order, so registrations keep their order.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;

use dictate_speech::CustomDictionary;
use dictate_speech::DictationContext;
use dictate_speech::DictationMode;
use dictate_speech::ProcessedDictation;
use dictate_speech::RawTranscript;
use dictate_speech::ReplacementRule;
use dictate_speech::SpokenFormatting;
use mlua::FromLuaMulti;
use mlua::Function;
use mlua::IntoLuaMulti;
use mlua::Lua;
use mlua::LuaSerdeExt;
use mlua::MultiValue;
use mlua::Table;
use mlua::Value;
use serde::Deserialize;
use serde::Serialize;

mod api;
mod core;
mod exec;
mod runtime;

pub use core::CoreCall;
pub use core::CoreReply;
pub use core::CoreResult;
pub use core::HotkeyPhase;
pub use core::PluginEvent;
pub use core::SessionId;

pub use api::lua_types;

const BUNDLED: [(&str, &str); 3] = [
    ("formatter", include_str!("../plugins/formatter.lua")),
    ("agent", include_str!("../plugins/agent.lua")),
    ("dictation", include_str!("../plugins/dictation.lua")),
];

const HOOKS: &str = "dictate.hooks";
const HOTKEY_HANDLERS: &str = "dictate.events.hotkey";
const RECORD_HANDLERS: &str = "dictate.events.record";
const SESSION_HANDLERS: &str = "dictate.events.session";
const EVENT_HANDLERS: [&str; 3] = [HOTKEY_HANDLERS, RECORD_HANDLERS, SESSION_HANDLERS];
const DICTIONARY: &str = "dictionary";
const REPLACEMENTS: &str = "replacements";
const REGISTRIES: [&str; 2] = [DICTIONARY, REPLACEMENTS];

/// One spoken-to-written pair in the dictionary or replacements.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, schemars::JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub spoken: String,
    pub written: String,
}

/// Configuration the registries start from, and where user plugins live.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginSetup {
    pub mode: DictationMode,
    pub spoken_formatting: Option<SpokenFormatting>,
    pub dictionary: Vec<Entry>,
    pub replacements: Vec<Entry>,
    pub plugins_dir: Option<PathBuf>,
    /// Each plugin's `[plugins.<id>]` table from `config.toml`, passed to the
    /// plugin file as its argument (`local options = ...`).
    pub options: BTreeMap<String, serde_json::Value>,
    /// Plugins with `enabled = false`; they are listed but never loaded.
    pub disabled: BTreeSet<String>,
}

/// What the core knows about where a transcript is going, for hooks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TranscriptContext {
    /// The focused application's identifier when recording stopped, if
    /// known. Only captured for insert delivery.
    app_id: Option<String>,
    window_title: Option<String>,
}

impl PluginSetup {
    /// The formatting context from configuration alone, before any plugin
    /// transforms.
    #[must_use]
    pub fn base_context(&self) -> DictationContext {
        dictation_context(
            self.mode,
            self.spoken_formatting,
            &self.dictionary,
            &self.replacements,
        )
    }
}

/// One loaded plugin and anything that went wrong while loading it or
/// rebuilding registries with it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PluginStatus {
    pub id: String,
    /// `bundled`, or the plugin file's path.
    pub source: String,
    /// Whether its registrations are live. A plugin whose file fails to
    /// load stays active on the last version that loaded.
    pub active: bool,
    pub errors: Vec<String>,
}

/// Called from a command's thread when a parked `dictate.exec` finished, so
/// the owner can [`Plugins::poll`].
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A core call plugin code is parked on; answer it with [`Plugins::reply`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CallId(u64);

/// Plugins loaded into one Lua VM, driven by the thread that owns it.
///
/// Work that waits (a command, a core call) parks its coroutine, so a
/// transcript hook waiting on an agent never holds up other plugin work.
/// Core calls collect in [`Self::take_calls`]; finished commands are picked
/// up by [`Self::poll`] after [`Wake`] fires.
pub struct Plugins {
    lua: Lua,
    setup: PluginSetup,
    /// Every plugin, in load order.
    slots: Vec<Slot>,
    wake: Wake,
    inbox_tx: mpsc::Sender<ExecDone>,
    inbox: mpsc::Receiver<ExecDone>,
    next_task: u64,
    /// Tasks parked on a call, by task id. A task parked on a daemon call
    /// uses its id as the call id.
    parked: HashMap<u64, (runtime::Task, Owner)>,
    next_chain: u64,
    /// Transcript chains in progress.
    chains: HashMap<u64, Chain>,
    /// Chains the daemon asked for that finished, by request id.
    finished: Vec<(u64, ProcessedDictation)>,
    /// Daemon calls made since the last [`Self::take_calls`].
    calls: Vec<(u64, core::CoreCall)>,
    /// Running commands' cancel flags, by the task waiting on them.
    execs: HashMap<u64, Arc<AtomicBool>>,
    /// Open recording sessions and the plugin that opened each.
    sessions: HashMap<core::SessionId, String>,
}

/// What a plugin's source looked like, to notice edits.
#[derive(Clone, Debug, PartialEq)]
enum Revision {
    Bundled,
    /// A hash of the file's contents, or of why it could not be read.
    File(u64),
}

/// A plugin in load order and what came of loading it.
struct Slot {
    status: PluginStatus,
    revision: Revision,
    /// The code whose registrations are live: the current source, or the
    /// last version that loaded when the current one fails. `None` while
    /// the plugin is inactive.
    running: Option<String>,
}

/// A command a parked task was waiting on ended.
struct ExecDone {
    task: u64,
    outcome: Result<api::ExecResult, String>,
}

/// What a task's result feeds into.
#[derive(Clone, Copy)]
enum Owner {
    /// One hook of a transcript chain.
    Chain(u64),
    /// An event handler; only failures matter.
    Event,
}

/// Who gets a chain's text.
#[derive(Clone, Copy)]
enum ChainReply {
    /// The daemon's request id.
    Daemon(u64),
    /// A task parked in `dictate.transcript.run`.
    Task(u64),
}

/// A run of the transcript hook chain.
struct Chain {
    reply: ChainReply,
    raw: String,
    context: TranscriptContext,
    text: String,
    hooks: VecDeque<Registration>,
}

/// One hook or transform, with the plugin that registered it.
struct Registration {
    plugin: String,
    function: Function,
}

/// Registrations are only accepted while a plugin file loads.
struct Loading(Cell<bool>);
/// The context `dictate.text.format` formats with.
struct EffectiveContext(RefCell<DictationContext>);

impl Plugins {
    /// Load the bundled plugins and every `*.lua` file in the plugins
    /// directory, then rebuild the registries.
    ///
    /// A plugin that fails to load is reported in [`Self::plugins`] and
    /// leaves no registrations behind; only a failure to set up the Lua VM
    /// itself is an error.
    pub fn load(setup: PluginSetup, wake: Wake) -> mlua::Result<Self> {
        let lua = Lua::new();
        install_api(&lua, setup.base_context())?;

        let (inbox_tx, inbox) = mpsc::channel();
        let mut host = Self {
            lua,
            setup,
            slots: Vec::new(),
            wake,
            inbox_tx,
            inbox,
            next_task: 1,
            parked: HashMap::new(),
            next_chain: 1,
            chains: HashMap::new(),
            finished: Vec::new(),
            calls: Vec::new(),
            execs: HashMap::new(),
            sessions: HashMap::new(),
        };
        host.reload_if_changed();
        Ok(host)
    }

    /// Every plugin in load order, with what went wrong loading it.
    #[must_use]
    pub fn plugins(&self) -> Vec<PluginStatus> {
        self.slots
            .iter()
            .map(|slot| PluginStatus {
                active: slot.running.is_some(),
                ..slot.status.clone()
            })
            .collect()
    }

    /// Reload after plugin files were added, removed, or changed. Plugins
    /// before the first changed one stay loaded; the rest are unloaded in
    /// reverse and loaded again in order. Returns whether anything changed.
    pub fn reload_if_changed(&mut self) -> bool {
        let sources = plugin_sources(plugin_files(self.setup.plugins_dir.as_deref()));
        let unchanged = self
            .slots
            .iter()
            .zip(&sources)
            .take_while(|(slot, source)| {
                slot.status.id == source.id && slot.revision == source.revision
            })
            .count();
        if unchanged == self.slots.len() && unchanged == sources.len() {
            return false;
        }

        let closed = self.slots.split_off(unchanged);
        for slot in closed.iter().rev() {
            self.close(&slot.status.id);
        }
        let mut closed = closed
            .into_iter()
            .map(|slot| (slot.status.id.clone(), slot))
            .collect::<HashMap<_, _>>();
        for source in sources.into_iter().skip(unchanged) {
            let previous = closed.remove(&source.id);
            let slot = self.open(&source, previous);
            self.slots.push(slot);
        }
        self.rebuild_registries();
        true
    }

    /// Load one plugin. A file that has not changed since `previous` is not
    /// run again if it failed; one that fails now leaves `previous`'s code
    /// running.
    fn open(&mut self, source: &PluginSource, previous: Option<Slot>) -> Slot {
        let mut slot = Slot {
            status: PluginStatus {
                id: source.id.clone(),
                source: source.source.clone(),
                active: false,
                errors: Vec::new(),
            },
            revision: source.revision.clone(),
            running: None,
        };
        if self.setup.disabled.contains(&source.id) {
            return slot;
        }
        let options = self.setup.options.get(&source.id);
        let (code, fallback) = match previous {
            Some(previous) if previous.revision == source.revision => {
                slot.status.errors = previous.status.errors;
                (previous.running.ok_or(None), None)
            }
            previous => (
                source.read().map_err(Some),
                previous.and_then(|previous| previous.running),
            ),
        };
        let loaded = code.and_then(|code| {
            load_plugin(&self.lua, &source.id, &source.source, &code, options)
                .map(|()| code)
                .map_err(Some)
        });
        match loaded {
            Ok(code) => slot.running = Some(code),
            Err(None) => {}
            Err(Some(error)) => {
                eprintln!("plugin {}: {error}", source.id);
                slot.status.errors.push(error);
                if let Some(code) = fallback {
                    match load_plugin(&self.lua, &source.id, &source.source, &code, options) {
                        Ok(()) => {
                            eprintln!(
                                "plugin {}: still running the version that loaded before",
                                source.id
                            );
                            slot.status
                                .errors
                                .push("still running the version that loaded before".to_owned());
                            slot.running = Some(code);
                        }
                        Err(error) => slot.status.errors.push(error),
                    }
                }
            }
        }
        slot
    }

    /// Unload `plugin`: drop its registrations, the work it is waiting on,
    /// and the recording sessions it opened.
    fn close(&mut self, plugin: &str) {
        if let Err(error) = drop_registrations(&self.lua, plugin) {
            eprintln!("plugin {plugin}: could not drop its registrations: {error}");
        }
        let tasks = self
            .parked
            .iter()
            .filter(|(_, (task, _))| task.plugin() == plugin)
            .map(|(&id, _)| id)
            .collect::<Vec<_>>();
        for id in tasks {
            let Some((_, owner)) = self.parked.remove(&id) else {
                continue;
            };
            if let Some(cancel) = self.execs.remove(&id) {
                cancel.store(true, Ordering::Relaxed);
            }
            if let Owner::Chain(_) = owner {
                self.complete(
                    owner,
                    plugin,
                    Err(mlua::Error::runtime("its plugin was unloaded")),
                );
            }
        }
        let sessions = self
            .sessions
            .iter()
            .filter(|(_, owner)| *owner == plugin)
            .map(|(&session, _)| session)
            .collect::<Vec<_>>();
        for session in sessions {
            self.sessions.remove(&session);
            self.cancel_session(session);
        }
    }

    /// Ask the core to cancel `session` for a plugin that is gone; nobody
    /// waits for the reply.
    fn cancel_session(&mut self, session: core::SessionId) {
        let id = self.next_task;
        self.next_task += 1;
        self.calls
            .push((id, core::CoreCall::SessionCancel { session }));
    }

    fn is_active(&self, plugin: &str) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.status.id == plugin && slot.running.is_some())
    }

    /// Resume tasks whose commands finished. Call after [`Wake`] fires.
    pub fn poll(&mut self) {
        while let Ok(done) = self.inbox.try_recv() {
            self.finish_exec(done.task, done.outcome);
        }
    }

    /// Core calls plugin code made since the last call.
    pub fn take_calls(&mut self) -> Vec<(CallId, core::CoreCall)> {
        std::mem::take(&mut self.calls)
            .into_iter()
            .map(|(task, call)| (CallId(task), call))
            .collect()
    }

    /// Run every handler for `event`, each in its own coroutine.
    pub fn dispatch(&mut self, event: &core::PluginEvent) {
        let (key, argument) = match event {
            core::PluginEvent::Hotkey { name, phase, focus } => (
                HOTKEY_HANDLERS,
                self.lua.to_value_with(
                    &api::HotkeyEvent {
                        name: name.clone(),
                        phase: *phase,
                        focus: focus.as_ref().map(api::Focus::from_snapshot),
                    },
                    json_to_lua(),
                ),
            ),
            core::PluginEvent::Record {
                command,
                target,
                focus,
            } => (
                RECORD_HANDLERS,
                self.lua.to_value_with(
                    &api::RecordEvent {
                        command: *command,
                        target: *target,
                        focus: focus.as_ref().map(api::Focus::from_snapshot),
                    },
                    json_to_lua(),
                ),
            ),
            core::PluginEvent::SessionEnded { session } => (
                SESSION_HANDLERS,
                self.lua.to_value_with(
                    &api::SessionEvent {
                        id: session.0,
                        kind: api::SessionEventKind::Ended,
                    },
                    json_to_lua(),
                ),
            ),
        };
        let (handlers, argument) = match (registrations(&self.lua, key), argument) {
            (Ok(handlers), Ok(argument)) => (handlers, argument),
            (Err(error), _) | (_, Err(error)) => {
                eprintln!("plugin event handlers are unavailable: {error}");
                return;
            }
        };
        for handler in handlers {
            match runtime::Task::new(&self.lua, &handler.plugin, &handler.function) {
                Ok(mut task) => {
                    let step = task.resume(&self.lua, argument.clone());
                    self.on_step(task, Owner::Event, step);
                }
                Err(error) => self.complete(Owner::Event, &handler.plugin, Err(error)),
            }
        }
    }

    /// Answer core call `call` and resume the task waiting on it. A reply
    /// for a task that is gone, for example after a reload, is dropped.
    pub fn reply(&mut self, call: CallId, result: core::CoreResult) {
        let opened = match &result {
            Ok(core::CoreReply::Session { session }) => Some(*session),
            _ => None,
        };
        let Some((mut task, owner)) = self.parked.remove(&call.0) else {
            // Opened for a plugin that was unloaded while it waited.
            if let Some(session) = opened {
                self.cancel_session(session);
            }
            return;
        };
        if let Some(session) = opened {
            self.sessions.insert(session, task.plugin().to_owned());
        }
        let step = match self.lua.create_userdata(runtime::CoreReplyValue(result)) {
            Ok(reply) => task.resume(&self.lua, reply),
            Err(error) => runtime::Step::Failed(error),
        };
        self.on_step(task, owner, step);
    }

    /// Run the transcript hook chain to completion on this thread, for the
    /// command line. Core calls fail: there is no daemon to serve them.
    pub fn process(
        &mut self,
        raw: &RawTranscript,
        transcript: &TranscriptContext,
    ) -> ProcessedDictation {
        const LOCAL: u64 = u64::MAX;
        self.start_chain(LOCAL, raw, transcript);
        loop {
            for (call, _) in self.take_calls() {
                self.reply(call, Err("core calls need the running daemon".to_owned()));
            }
            if let Some(index) = self.finished.iter().position(|(id, _)| *id == LOCAL) {
                return self.finished.remove(index).1;
            }
            if !self.calls.is_empty() {
                continue;
            }
            // The host holds a sender itself, so the channel never
            // disconnects.
            if let Ok(done) = self.inbox.recv() {
                self.finish_exec(done.task, done.outcome);
            }
        }
    }

    /// Start the transcript hook chain for request `id`; its result appears
    /// in [`Self::take_finished`]. A hook that errors, overruns its budget,
    /// or returns something other than a string or nil is skipped and the
    /// chain continues with the text it was given.
    fn start_chain(&mut self, id: u64, raw: &RawTranscript, transcript: &TranscriptContext) {
        self.run_chain(ChainReply::Daemon(id), raw.as_str(), transcript);
    }

    fn run_chain(&mut self, reply: ChainReply, raw: &str, transcript: &TranscriptContext) {
        let hooks = registrations(&self.lua, HOOKS).unwrap_or_else(|error| {
            eprintln!("plugin hooks are unavailable, using the raw transcript: {error}");
            Vec::new()
        });
        let id = self.next_chain;
        self.next_chain += 1;
        self.chains.insert(
            id,
            Chain {
                reply,
                raw: raw.to_owned(),
                context: transcript.clone(),
                text: raw.to_owned(),
                hooks: hooks.into(),
            },
        );
        self.advance_chain(id);
    }

    /// Start the chain's next hook, or finish the chain.
    fn advance_chain(&mut self, id: u64) {
        let next = loop {
            let Some(chain) = self.chains.get_mut(&id) else {
                return;
            };
            match chain.hooks.pop_front() {
                // Its plugin was unloaded or disabled since the chain started.
                Some(hook) if !self.is_active(&hook.plugin) => {}
                next => break next,
            }
        };
        let Some(chain) = self.chains.get_mut(&id) else {
            return;
        };
        let Some(hook) = next else {
            let text = chain.text.trim().to_owned();
            let reply = chain.reply;
            self.chains.remove(&id);
            self.finish_chain(reply, text);
            return;
        };
        let context = api::HookContext {
            raw: chain.raw.clone(),
            mode: self.setup.mode,
            app: (chain.context.app_id.is_some() || chain.context.window_title.is_some()).then(
                || api::App {
                    id: chain.context.app_id.clone(),
                    title: chain.context.window_title.clone(),
                },
            ),
        };
        let text = chain.text.clone();
        let started = self
            .lua
            .to_value_with(&context, json_to_lua())
            .and_then(|context| {
                let mut task = runtime::Task::new(&self.lua, &hook.plugin, &hook.function)?;
                let step = task.resume(&self.lua, (text, context));
                Ok((task, step))
            });
        match started {
            Ok((task, step)) => self.on_step(task, Owner::Chain(id), step),
            Err(error) => self.complete(Owner::Chain(id), &hook.plugin, Err(error)),
        }
    }

    fn finish_chain(&mut self, reply: ChainReply, text: String) {
        match reply {
            ChainReply::Daemon(id) => self.finished.push((id, ProcessedDictation::new(text))),
            ChainReply::Task(task_id) => {
                let Some((mut task, owner)) = self.parked.remove(&task_id) else {
                    return;
                };
                let step = task.resume(&self.lua, text);
                self.on_step(task, owner, step);
            }
        }
    }

    /// A command a parked task was waiting on ended; resume the task.
    fn finish_exec(&mut self, task_id: u64, outcome: Result<api::ExecResult, String>) {
        self.execs.remove(&task_id);
        let Some((mut task, owner)) = self.parked.remove(&task_id) else {
            return;
        };
        let step = match exec::into_lua(&self.lua, outcome) {
            Ok(values) => task.resume(&self.lua, values),
            Err(error) => runtime::Step::Failed(error),
        };
        self.on_step(task, owner, step);
    }

    fn on_step(&mut self, task: runtime::Task, owner: Owner, step: runtime::Step) {
        match step {
            runtime::Step::Parked(call) => self.park(task, owner, call),
            runtime::Step::Done(values) => self.complete(owner, task.plugin(), Ok(values)),
            runtime::Step::Failed(error) => self.complete(owner, task.plugin(), Err(error)),
        }
    }

    /// Start the work `call` waits for and park `task` until it is done.
    fn park(&mut self, task: runtime::Task, owner: Owner, call: runtime::Call) {
        let task_id = self.next_task;
        self.next_task += 1;
        // Park first: a nested chain may finish, and resume the task, at once.
        self.parked.insert(task_id, (task, owner));
        match call {
            runtime::Call::Core(op) => {
                if let core::CoreCall::SessionStop { session, .. }
                | core::CoreCall::SessionCancel { session } = &op
                {
                    self.sessions.remove(session);
                }
                self.calls.push((task_id, op));
            }
            runtime::Call::Chain { text, context } => {
                self.run_chain(ChainReply::Task(task_id), &text, &context);
            }
            runtime::Call::Exec(request) => {
                let cancel = Arc::new(AtomicBool::new(false));
                self.execs.insert(task_id, Arc::clone(&cancel));
                let inbox = self.inbox_tx.clone();
                let wake = Arc::clone(&self.wake);
                thread::spawn(move || {
                    let outcome = exec::run(&request, &cancel);
                    if inbox
                        .send(ExecDone {
                            task: task_id,
                            outcome,
                        })
                        .is_ok()
                    {
                        wake();
                    }
                });
            }
        }
    }

    fn complete(&mut self, owner: Owner, plugin: &str, result: mlua::Result<MultiValue>) {
        match owner {
            Owner::Chain(id) => {
                if let Some(chain) = self.chains.get_mut(&id) {
                    let returned =
                        result.map(|values| values.into_iter().next().unwrap_or(Value::Nil));
                    match returned {
                        Ok(Value::String(next)) => chain.text = next.to_string_lossy(),
                        Ok(Value::Nil) => {}
                        Ok(other) => eprintln!(
                            "plugin {plugin}: transcript hook returned a {}, expected a string or nil; keeping the previous text",
                            other.type_name()
                        ),
                        Err(error) => eprintln!(
                            "plugin {plugin}: transcript hook failed; keeping the previous text: {error}"
                        ),
                    }
                }
                self.advance_chain(id);
            }
            Owner::Event => {
                if let Err(error) = result {
                    eprintln!("plugin {plugin}: event handler failed: {error}");
                }
            }
        }
    }

    /// Rebuild every registry. A plugin whose transform fails is disabled
    /// and the rebuild starts over without it.
    fn rebuild_registries(&mut self) {
        let context = loop {
            match self.try_rebuild() {
                Ok(context) => break context,
                Err((plugin, error)) => self.disable(&plugin, &error),
            }
        };
        if let Some(effective) = self.lua.app_data_ref::<EffectiveContext>() {
            *effective.0.borrow_mut() = context;
        }
    }

    fn try_rebuild(&self) -> Result<DictationContext, (String, String)> {
        let dictionary = self.rebuild_registry(DICTIONARY, &self.setup.dictionary)?;
        let replacements = self.rebuild_registry(REPLACEMENTS, &self.setup.replacements)?;
        Ok(dictation_context(
            self.setup.mode,
            self.setup.spoken_formatting,
            &dictionary,
            &replacements,
        ))
    }

    /// The registry rebuilt from `initial`, or the plugin whose transform
    /// failed and why.
    fn rebuild_registry(
        &self,
        name: &str,
        initial: &[Entry],
    ) -> Result<Vec<Entry>, (String, String)> {
        let transforms = registrations(&self.lua, &transforms_key(name)).unwrap_or_else(|error| {
            eprintln!("{name} transforms are unavailable: {error}");
            Vec::new()
        });
        let mut current = initial.to_vec();
        for Registration { plugin, function } in transforms {
            let rebuilt = self.lua.to_value(&current).and_then(|value| {
                let returned = call_plugin::<Value>(&self.lua, &plugin, &function, value.clone())?;
                let rebuilt = if returned.is_nil() { value } else { returned };
                self.lua.from_value::<Vec<Entry>>(rebuilt)
            });
            match rebuilt {
                Ok(rebuilt) => current = rebuilt,
                Err(error) => return Err((plugin, format!("{name} transform failed: {error}"))),
            }
        }
        Ok(current)
    }

    fn disable(&mut self, plugin: &str, error: &str) {
        let error = format!("disabled: {error}");
        eprintln!("plugin {plugin}: {error}");
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.status.id == plugin) {
            slot.running = None;
            slot.status.errors.push(error);
        }
        self.close(plugin);
    }
}

struct PluginSource {
    id: String,
    /// `bundled`, or the file's path.
    source: String,
    revision: Revision,
    code: Code,
}

enum Code {
    Bundled(&'static str),
    /// The file's contents, or why it could not be read.
    File(Result<String, String>),
}

impl PluginSource {
    fn read(&self) -> Result<String, String> {
        match &self.code {
            Code::Bundled(code) => Ok((*code).to_owned()),
            Code::File(code) => code.clone(),
        }
    }
}

/// Bundled plugins first, in bundled order; a user file with a bundled
/// plugin's name takes that plugin's place. Remaining user files follow in
/// file-name order.
fn plugin_sources(files: Vec<(PathBuf, Result<String, String>)>) -> Vec<PluginSource> {
    let mut user = files
        .into_iter()
        .map(|(path, code)| PluginSource {
            id: path
                .file_stem()
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned()),
            source: path.display().to_string(),
            revision: Revision::File(std::hash::BuildHasher::hash_one(
                &std::hash::BuildHasherDefault::<std::hash::DefaultHasher>::default(),
                &code,
            )),
            code: Code::File(code),
        })
        .collect::<Vec<_>>();

    let mut sources = Vec::new();
    for (id, code) in BUNDLED {
        if let Some(index) = user.iter().position(|source| source.id == id) {
            sources.push(user.remove(index));
        } else {
            sources.push(PluginSource {
                id: id.to_owned(),
                source: "bundled".to_owned(),
                revision: Revision::Bundled,
                code: Code::Bundled(code),
            });
        }
    }
    sources.extend(user);
    sources
}

/// Run a plugin file's top level. On failure, whatever it registered
/// before failing is dropped, so a half-loaded plugin never runs.
fn load_plugin(
    lua: &Lua,
    id: &str,
    source: &str,
    code: &str,
    options: Option<&serde_json::Value>,
) -> Result<(), String> {
    set_loading(lua, true);
    let result = lua
        .load(code)
        .set_name(format!("@{source}"))
        .into_function()
        .and_then(|chunk| {
            let options = match options {
                Some(options) => lua.to_value_with(options, json_to_lua())?,
                None => Value::Nil,
            };
            call_plugin::<()>(lua, id, &chunk, options)
        });
    set_loading(lua, false);
    result.map_err(|error| {
        if let Err(cleanup) = drop_registrations(lua, id) {
            eprintln!("plugin {id}: cleanup failed: {cleanup}");
        }
        format!("load failed: {error}")
    })
}

fn install_api(lua: &Lua, context: DictationContext) -> mlua::Result<()> {
    runtime::install(lua);
    lua.set_app_data(Loading(Cell::new(false)));
    lua.set_app_data(EffectiveContext(RefCell::new(context)));

    lua.set_named_registry_value(HOOKS, lua.create_table()?)?;
    for key in EVENT_HANDLERS {
        lua.set_named_registry_value(key, lua.create_table()?)?;
    }
    for name in REGISTRIES {
        lua.set_named_registry_value(&transforms_key(name), lua.create_table()?)?;
    }
    api::install(lua)
}

/// JSON `null` becomes `nil`, so plugins can test fields with `if`.
fn json_to_lua() -> mlua::serde::SerializeOptions {
    mlua::serde::SerializeOptions::new()
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false)
}

fn register(lua: &Lua, key: &str, function: Function) -> mlua::Result<()> {
    let loading = lua
        .app_data_ref::<Loading>()
        .is_some_and(|loading| loading.0.get());
    let Some(plugin) = runtime::active_plugin(lua).filter(|_| loading) else {
        return Err(mlua::Error::runtime(
            "hooks and transforms can only be registered while a plugin file loads",
        ));
    };
    let entry = lua.create_table()?;
    entry.set("plugin", plugin)?;
    entry.set("fn", function)?;
    lua.named_registry_value::<Table>(key)?.push(entry)
}

fn registrations(lua: &Lua, key: &str) -> mlua::Result<Vec<Registration>> {
    lua.named_registry_value::<Table>(key)?
        .sequence_values::<Table>()
        .map(|entry| {
            let entry = entry?;
            Ok(Registration {
                plugin: entry.get("plugin")?,
                function: entry.get("fn")?,
            })
        })
        .collect()
}

fn drop_registrations(lua: &Lua, plugin: &str) -> mlua::Result<()> {
    let keys = std::iter::once(HOOKS.to_owned())
        .chain(EVENT_HANDLERS.into_iter().map(str::to_owned))
        .chain(REGISTRIES.into_iter().map(transforms_key));
    for key in keys {
        let kept = lua.create_table()?;
        for entry in lua
            .named_registry_value::<Table>(&key)?
            .sequence_values::<Table>()
        {
            let entry = entry?;
            if entry.get::<String>("plugin")? != plugin {
                kept.push(entry)?;
            }
        }
        lua.set_named_registry_value(&key, kept)?;
    }
    Ok(())
}

/// Run plugin code to completion with `plugin` marked active; core calls it
/// makes are resolved on this thread.
fn call_plugin<R: FromLuaMulti>(
    lua: &Lua,
    plugin: &str,
    function: &Function,
    args: impl IntoLuaMulti,
) -> mlua::Result<R> {
    let values = runtime::Task::new(lua, plugin, function)?.run_blocking(lua, args)?;
    R::from_lua_multi(values, lua)
}

fn set_loading(lua: &Lua, loading: bool) {
    if let Some(flag) = lua.app_data_ref::<Loading>() {
        flag.0.set(loading);
    }
}

fn transforms_key(registry: &str) -> String {
    format!("dictate.transforms.{registry}")
}

fn dictation_context(
    mode: DictationMode,
    spoken_formatting: Option<SpokenFormatting>,
    dictionary: &[Entry],
    replacements: &[Entry],
) -> DictationContext {
    let mut context = DictationContext::new(mode);
    if let Some(spoken_formatting) = spoken_formatting {
        context = context.with_spoken_formatting(spoken_formatting);
    }
    if !dictionary.is_empty() {
        context = context.with_dictionary(CustomDictionary::from_entries(
            dictionary
                .iter()
                .map(|entry| (entry.spoken.as_str(), entry.written.as_str())),
        ));
    }
    if !replacements.is_empty() {
        context = context.with_replacement_rules(
            replacements
                .iter()
                .map(|entry| ReplacementRule::new(entry.spoken.as_str(), entry.written.as_str()))
                .collect(),
        );
    }
    context
}

/// The `*.lua` files in `dir`, sorted by path, with their contents. Read
/// in full every time: comparing contents notices an edit that keeps the
/// file's size and modification time.
fn plugin_files(dir: Option<&Path>) -> Vec<(PathBuf, Result<String, String>)> {
    let Some(entries) = dir.and_then(|dir| fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "lua"))
        .filter(|path| path.is_file())
        .map(|path| {
            let code = fs::read_to_string(&path).map_err(|error| format!("read failed: {error}"));
            (path, code)
        })
        .collect::<Vec<_>>();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use std::time::Instant;

    use super::*;

    static TEST_DIR_ID: AtomicUsize = AtomicUsize::new(0);

    struct PluginDir(PathBuf);

    impl PluginDir {
        fn new(files: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "dictate-plugins-{}-{}",
                std::process::id(),
                TEST_DIR_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).expect("plugin dir should be created");
            let dir = Self(dir);
            for (name, code) in files {
                dir.write(name, code);
            }
            dir
        }

        fn write(&self, name: &str, code: &str) {
            fs::write(self.0.join(name), code).expect("plugin file should be written");
        }

        fn host(&self) -> Plugins {
            load(setup(Some(self.0.clone())))
        }
    }

    impl Drop for PluginDir {
        fn drop(&mut self) {
            drop(fs::remove_dir_all(&self.0));
        }
    }

    fn setup(plugins_dir: Option<PathBuf>) -> PluginSetup {
        PluginSetup {
            mode: DictationMode::Message,
            spoken_formatting: None,
            dictionary: vec![Entry {
                spoken: "gee pee you eye".to_owned(),
                written: "GPUI".to_owned(),
            }],
            replacements: Vec::new(),
            plugins_dir,
            options: BTreeMap::new(),
            disabled: BTreeSet::new(),
        }
    }

    fn load(setup: PluginSetup) -> Plugins {
        Plugins::load(setup, Arc::new(|| {})).expect("plugins should load")
    }

    fn process(host: &mut Plugins, raw: &str) -> String {
        host.process(&RawTranscript::new(raw), &TranscriptContext::default())
            .as_str()
            .to_owned()
    }

    fn status(host: &Plugins, id: &str) -> PluginStatus {
        host.plugins()
            .into_iter()
            .find(|status| status.id == id)
            .expect("plugin should be listed")
    }

    fn errors(host: &Plugins, id: &str) -> Vec<String> {
        status(host, id).errors
    }

    #[test]
    fn bundled_formatter_formats_with_the_configured_dictionary() {
        let mut host = load(setup(None));

        assert_eq!(
            process(&mut host, "um gee pee you eye works comma right period"),
            "GPUI works, right."
        );
        assert_eq!(host.plugins()[0].source, "bundled");
    }

    #[test]
    fn hooks_run_after_the_formatter_in_file_order_and_failures_keep_the_text() {
        let dir = PluginDir::new(&[
            (
                "b-shout.lua",
                "dictate.on('transcript', function(text) return text:upper() end)",
            ),
            (
                "a-fail.lua",
                "dictate.on('transcript', function() error('boom') end)",
            ),
            (
                "c-number.lua",
                "dictate.on('transcript', function() return 42 end)",
            ),
            (
                "d-mark.lua",
                "dictate.on('transcript', function(text, ctx) return text .. ' [' .. ctx.mode .. ']' end)",
            ),
        ]);
        let mut host = dir.host();

        assert_eq!(
            process(&mut host, "hello comma world period"),
            "HELLO, WORLD. [message]"
        );
    }

    #[test]
    fn transforms_get_fresh_copies_and_a_bad_result_disables_its_plugin() {
        let dir = PluginDir::new(&[
            (
                "a-add.lua",
                "local seen = nil
                 dictate.dictionary.transform(function(entries)
                   seen = entries
                   table.insert(entries, { spoken = 'rust lang', written = 'Rust' })
                 end)
                 dictate.dictionary.transform(function(entries)
                   -- Mutating the table another transform kept must not leak.
                   seen[1] = { spoken = 'gee pee you eye', written = 'broken' }
                 end)",
            ),
            (
                "b-bad.lua",
                "dictate.replacements.transform(function(entries)
                   table.insert(entries, { spoken = 'and', written = '&' })
                 end)
                 dictate.on('transcript', function(text) return text .. ' (bad ran)' end)
                 dictate.dictionary.transform(function() return { { spoken = 'x' } } end)",
            ),
        ]);
        let mut host = dir.host();

        // Neither b-bad's replacement, applied before its failing transform,
        // nor its hook survives.
        assert_eq!(
            process(&mut host, "gee pee you eye and rust lang"),
            "GPUI and Rust"
        );
        let bad = errors(&host, "b-bad");
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(
            bad[0].contains("disabled: dictionary transform failed"),
            "{bad:?}"
        );
        assert!(!status(&host, "b-bad").active);
    }

    #[test]
    fn a_plugin_that_fails_to_load_leaves_no_registrations() {
        let dir = PluginDir::new(&[(
            "half.lua",
            "dictate.on('transcript', function() return 'hijacked' end)
             error('fails after registering')",
        )]);
        let mut host = dir.host();

        assert_eq!(process(&mut host, "hello period"), "Hello.");
        assert!(errors(&host, "half")[0].contains("fails after registering"));
    }

    #[test]
    fn registration_outside_loading_is_rejected() {
        let dir = PluginDir::new(&[(
            "late.lua",
            "dictate.on('transcript', function(text)
               dictate.on('transcript', function() return 'late' end)
               return text
             end)",
        )]);
        let mut host = dir.host();

        assert_eq!(process(&mut host, "one period"), "One.");
        assert_eq!(process(&mut host, "two period"), "Two.");
    }

    #[test]
    fn a_user_formatter_replaces_the_bundled_one_in_its_slot() {
        let dir = PluginDir::new(&[
            (
                "aaa.lua",
                "dictate.on('transcript', function(text) return text .. '?' end)",
            ),
            (
                "formatter.lua",
                "dictate.on('transcript', function(text) return '<' .. text .. '>' end)",
            ),
        ]);
        let mut host = dir.host();

        assert_eq!(process(&mut host, "hello period"), "<hello period>?");
        assert_eq!(host.plugins().len(), 4, "formatter, agent, dictation, aaa");
    }

    #[cfg(unix)]
    #[test]
    fn exec_returns_output_and_reports_programs_that_cannot_run() {
        let dir = PluginDir::new(&[(
            "shell.lua",
            "dictate.on('transcript', function(text)
               local echoed = assert(dictate.exec({ 'sh', '-c', 'cat; echo err >&2; exit 3' }, { stdin = text }))
               local missing, why = dictate.exec({ 'no-such-program-for-dictate' })
               return table.concat({
                 echoed.stdout, echoed.stderr, tostring(echoed.code), tostring(echoed.timed_out),
                 tostring(missing), why:match('could not run') or why,
               }, '|')
             end)",
        )]);
        let mut host = dir.host();

        assert_eq!(
            process(&mut host, "hi"),
            "Hi|err\n|3|false|nil|could not run"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exec_timeout_kills_the_command_and_what_it_started() {
        let pid_file = std::env::temp_dir().join(format!(
            "dictate-exec-timeout-{}-{}.pid",
            std::process::id(),
            TEST_DIR_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let code = format!(
            "dictate.on('transcript', function()
               local result = dictate.exec(
                 {{ 'sh', '-c', 'sleep 30 & echo $! > {}; wait' }},
                 {{ timeout_ms = 300 }}
               )
               return tostring(result.timed_out) .. ' ' .. tostring(result.code)
             end)",
            pid_file.display()
        );
        let dir = PluginDir::new(&[("slow.lua", &code)]);
        let mut host = dir.host();
        let started = Instant::now();

        assert_eq!(process(&mut host, "x"), "true nil");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let pid = fs::read_to_string(&pid_file).expect("command should record its child");
        drop(fs::remove_file(&pid_file));
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(!alive, "the command's child outlived the timeout");
    }

    #[cfg(unix)]
    #[test]
    fn a_chain_waiting_on_a_command_does_not_hold_up_another() {
        let dir = PluginDir::new(&[(
            "slow.lua",
            "dictate.on('transcript', function(text)
               if text:find('Slow') then dictate.exec({ 'sleep', '0.5' }) end
               return text .. ' done'
             end)",
        )]);
        let mut host = dir.host();
        let started = Instant::now();

        host.start_chain(
            1,
            &RawTranscript::new("slow"),
            &TranscriptContext::default(),
        );
        host.start_chain(
            2,
            &RawTranscript::new("fast"),
            &TranscriptContext::default(),
        );
        let finished = std::mem::take(&mut host.finished);
        assert_eq!(finished.len(), 1, "only the fast chain should be done");
        assert_eq!((finished[0].0, finished[0].1.as_str()), (2, "Fast done"));
        assert!(started.elapsed() < Duration::from_millis(400));

        let done = host
            .inbox
            .recv()
            .expect("the slow chain should be woken by its command");
        host.finish_exec(done.task, done.outcome);
        assert_eq!(host.finished.len(), 1);
        assert_eq!(
            (host.finished[0].0, host.finished[0].1.as_str()),
            (1, "Slow done")
        );
    }

    #[test]
    fn plugins_cannot_yield_the_hook_or_call_from_their_own_coroutines() {
        let dir = PluginDir::new(&[
            (
                "a-yield.lua",
                "dictate.on('transcript', function() coroutine.yield('x') return 'yielded' end)",
            ),
            (
                "b-nested.lua",
                "dictate.on('transcript', function(text)
                   local ok, why = pcall(coroutine.wrap(function() return dictate.exec({ 'true' }) end))
                   return text .. ' ' .. tostring(ok) .. ' ' .. (tostring(why):match('coroutine the plugin created') or tostring(why))
                 end)",
            ),
        ]);
        let mut host = dir.host();

        assert_eq!(
            process(&mut host, "hi"),
            "Hi false coroutine the plugin created"
        );
    }

    /// The bundled plugins with `[plugins.agent]` set to `options`, run on
    /// `raw` as if dictated into a focused editor window.
    #[cfg(unix)]
    fn run_agent(options: serde_json::Value, raw: &str) -> String {
        let mut setup = setup(None);
        setup.options.insert("agent".to_owned(), options);
        let mut host = load(setup);
        assert!(
            host.plugins().iter().all(|plugin| plugin.errors.is_empty()),
            "{:?}",
            host.plugins()
        );
        let context = TranscriptContext {
            app_id: Some("dev.zed.Zed".to_owned()),
            window_title: Some("notes.md".to_owned()),
        };
        host.process(&RawTranscript::new(raw), &context)
            .as_str()
            .to_owned()
    }

    /// An executable shell script standing in for an agent CLI.
    #[cfg(unix)]
    fn fake_agent(dir: &PluginDir, name: &str, script: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.0.join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("fake agent should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("fake agent should be executable");
        path.display().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn the_agent_plugin_does_nothing_unless_configured() {
        let mut host = load(setup(None));

        assert_eq!(process(&mut host, "hello period"), "Hello.");
        assert!(host.plugins().iter().any(|plugin| plugin.id == "agent"));
    }

    #[cfg(unix)]
    #[test]
    fn the_pi_backend_runs_one_shot_without_tools_and_uses_the_reply() {
        let dir = PluginDir::new(&[]);
        // Echo the arguments, then the prompt's last line (the text).
        let pi = fake_agent(&dir, "pi", r#"printf '%s ' "$@"; tail -n 1"#);

        let reply = run_agent(
            serde_json::json!({ "backend": "pi", "program": pi, "model": "anthropic/haiku" }),
            "gee pee you eye works period",
        );

        assert_eq!(
            reply,
            "-p --no-session --no-tools --model anthropic/haiku GPUI works."
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_prompt_names_the_app_and_the_mode() {
        let dir = PluginDir::new(&[]);
        let pi = fake_agent(&dir, "pi", "head -n 1");

        let reply = run_agent(serde_json::json!({ "backend": "pi", "program": pi }), "hi");

        assert!(reply.contains("inserted into notes.md."), "{reply}");
        assert!(reply.contains("It is a chat message"), "{reply}");
    }

    #[cfg(unix)]
    #[test]
    fn the_opencode_backend_posts_to_the_server_and_reads_data_text() {
        let dir = PluginDir::new(&[]);
        // Stand-in for curl: check the URL and body, then answer like the
        // server's generate API.
        let curl = fake_agent(
            &dir,
            "curl",
            r#"body=$(cat)
case "$*" in *http://127.0.0.1:4096/api/experimental/generate) ;; *) echo "bad url: $*" >&2; exit 2 ;; esac
case "$body" in *'"providerID":"anthropic"'*'"id":"haiku"'*|*'"id":"haiku"'*'"providerID":"anthropic"'*) ;; *) echo "bad body: $body" >&2; exit 3 ;; esac
printf '{"data":{"text":"From the server."}}'"#,
        );

        let reply = run_agent(
            serde_json::json!({
                "backend": "opencode",
                "program": curl,
                "server": "http://127.0.0.1:4096/",
                "model": "anthropic/haiku",
            }),
            "hello",
        );

        assert_eq!(reply, "From the server.");
    }

    #[cfg(unix)]
    #[test]
    fn the_command_backend_gets_json_and_failures_keep_the_text() {
        let dir = PluginDir::new(&[]);
        let script = fake_agent(&dir, "cleanup", "cat");

        let reply = run_agent(
            serde_json::json!({ "backend": "command", "command": [script] }),
            "hello period",
        );
        let request: serde_json::Value =
            serde_json::from_str(&reply).expect("the command should get JSON");
        assert_eq!(request["text"], "Hello.");
        assert_eq!(request["raw"], "hello period");
        assert_eq!(request["mode"], "message");
        assert_eq!(request["app"]["id"], "dev.zed.Zed");

        let failing = fake_agent(&dir, "failing", "echo partial; exit 1");
        assert_eq!(
            run_agent(
                serde_json::json!({ "backend": "command", "command": [failing] }),
                "hello period"
            ),
            "Hello."
        );
    }

    #[test]
    fn the_committed_type_definitions_are_current() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("meta/dictate.lua");
        let generated = lua_types();
        if std::env::var_os("DICTATE_UPDATE_LUA_TYPES").is_some() {
            fs::write(&path, &generated).expect("type definitions should be written");
        }
        let committed = fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == generated,
            "meta/dictate.lua is stale; regenerate it with DICTATE_UPDATE_LUA_TYPES=1 cargo test -p dictate-plugins"
        );
    }

    /// A plugin VM driven the way the daemon drives it, against a fake
    /// core. `core` answers each call and returns events to dispatch after
    /// its reply. Returns every call made.
    fn drive(
        host: &mut Plugins,
        events: &[PluginEvent],
        mut core: impl FnMut(&CoreCall) -> (CoreResult, Vec<PluginEvent>),
    ) -> Vec<CoreCall> {
        let mut made = Vec::new();
        let mut events = VecDeque::from(events.to_vec());
        while let Some(event) = events.pop_front() {
            host.dispatch(&event);
            loop {
                let calls = host.take_calls();
                if calls.is_empty() {
                    break;
                }
                for (call, op) in calls {
                    let (result, then) = core(&op);
                    made.push(op);
                    host.reply(call, result);
                    events.extend(then);
                }
            }
        }
        made
    }

    fn stop_focus() -> dictate_desktop::FocusSnapshot {
        dictate_desktop::FocusSnapshot::Focused(dictate_desktop::FocusedWindow::niri(
            dictate_desktop::NiriInstanceId::new(7, 1),
            42,
            Some("dev.zed.Zed"),
            Some("notes.md"),
        ))
    }

    fn hotkey(phase: HotkeyPhase) -> PluginEvent {
        PluginEvent::Hotkey {
            name: "dictate".to_owned(),
            phase,
            focus: None,
        }
    }

    fn record(
        command: dictate_speech::DictationCommand,
        target: Option<dictate_desktop::DeliveryTarget>,
    ) -> PluginEvent {
        PluginEvent::Record {
            command,
            target,
            focus: None,
        }
    }

    /// What was delivered: text, target, focus.
    type Delivered = (
        String,
        Option<dictate_desktop::DeliveryTarget>,
        Option<dictate_desktop::FocusSnapshot>,
    );

    /// The bundled `dictation.lua` against a core that hears `heard`.
    /// `after_open` is dispatched once the session is open; `ended` makes
    /// the recording end on its own right after that. Returns the
    /// deliveries and every call made.
    fn dictate_once(
        events: &[PluginEvent],
        after_open: &[PluginEvent],
        ended: bool,
        heard: &str,
    ) -> (Vec<Delivered>, Vec<CoreCall>) {
        let mut host = load(setup(None));
        let mut delivered = Vec::new();
        let mut opened = false;
        let calls = drive(&mut host, events, |call| match call {
            CoreCall::SessionOpen => {
                assert!(!opened, "one recording at a time");
                opened = true;
                let mut then = after_open.to_vec();
                if ended {
                    then.push(PluginEvent::SessionEnded {
                        session: SessionId(9),
                    });
                }
                (
                    Ok(CoreReply::Session {
                        session: SessionId(9),
                    }),
                    then,
                )
            }
            CoreCall::SessionStop { session, .. } => {
                assert_eq!(*session, SessionId(9));
                let transcript = CoreReply::Transcript {
                    text: heard.to_owned(),
                    focus: stop_focus(),
                };
                (Ok(transcript), Vec::new())
            }
            CoreCall::DesktopDeliver {
                text,
                target,
                focus,
            } => {
                delivered.push((text.clone(), *target, focus.clone()));
                let delivered = CoreReply::Delivered {
                    target: dictate_desktop::DeliveryTarget::Insert,
                };
                (Ok(delivered), Vec::new())
            }
            CoreCall::OverlayHide | CoreCall::SessionCancel { .. } => {
                (Ok(CoreReply::Done), Vec::new())
            }
            CoreCall::DesktopFocus => (Err("unexpected".to_owned()), Vec::new()),
        });
        (delivered, calls)
    }

    #[test]
    fn bundled_dictation_formats_and_delivers_to_the_stop_focus() {
        let (delivered, calls) = dictate_once(
            &[hotkey(HotkeyPhase::Down)],
            &[hotkey(HotkeyPhase::Down), hotkey(HotkeyPhase::Up)],
            false,
            "gee pee you eye works comma right period",
        );

        assert_eq!(
            delivered,
            vec![("GPUI works, right.".to_owned(), None, Some(stop_focus()))]
        );
        assert_eq!(calls.len(), 3, "open, stop, deliver: {calls:?}");
    }

    #[test]
    fn bundled_dictation_hides_the_overlay_when_nothing_was_heard() {
        let (delivered, calls) = dictate_once(
            &[hotkey(HotkeyPhase::Down)],
            &[hotkey(HotkeyPhase::Up)],
            false,
            "",
        );
        assert!(delivered.is_empty());
        assert_eq!(calls.last(), Some(&CoreCall::OverlayHide));
    }

    #[test]
    fn record_toggle_delivers_to_the_latest_delivery_target_given() {
        use dictate_desktop::DeliveryTarget::Clipboard;
        use dictate_desktop::DeliveryTarget::Stdout;
        use dictate_speech::DictationCommand::Toggle;
        for (at_start, at_stop, expected) in [
            (Some(Clipboard), None, Some(Clipboard)),
            (Some(Clipboard), Some(Stdout), Some(Stdout)),
            (None, None, None),
        ] {
            let (delivered, _) = dictate_once(
                &[record(Toggle, at_start)],
                &[record(Toggle, at_stop)],
                false,
                "hi period",
            );
            assert_eq!(
                delivered,
                vec![("Hi.".to_owned(), expected, Some(stop_focus()))],
                "{at_start:?} then {at_stop:?}"
            );
        }
    }

    #[test]
    fn a_recording_that_ends_on_its_own_is_delivered_without_a_release() {
        let (delivered, calls) =
            dictate_once(&[hotkey(HotkeyPhase::Down)], &[], true, "long period");
        assert_eq!(delivered.len(), 1, "{calls:?}");
        assert_eq!(delivered[0].0, "Long.");

        // The release after that does not stop anything again.
        let (_, calls) = dictate_once(
            &[hotkey(HotkeyPhase::Down)],
            &[hotkey(HotkeyPhase::Up)],
            true,
            "long period",
        );
        let stops = calls
            .iter()
            .filter(|call| matches!(call, CoreCall::SessionStop { .. }))
            .count();
        assert_eq!(stops, 1, "{calls:?}");
    }

    #[test]
    fn record_cancel_cancels_the_session() {
        use dictate_speech::DictationCommand::Cancel;
        use dictate_speech::DictationCommand::Start;
        let (delivered, calls) =
            dictate_once(&[record(Start, None)], &[record(Cancel, None)], false, "x");
        assert!(delivered.is_empty());
        assert_eq!(
            calls.last(),
            Some(&CoreCall::SessionCancel {
                session: SessionId(9)
            })
        );
    }

    #[test]
    fn reload_picks_up_edits_and_deletes() {
        let dir = PluginDir::new(&[(
            "tag.lua",
            "dictate.on('transcript', function(text) return text .. ' v1' end)",
        )]);
        let mut host = dir.host();
        assert_eq!(process(&mut host, "x"), "X v1");
        assert!(!host.reload_if_changed());

        dir.write(
            "tag.lua",
            "dictate.on('transcript', function(text) return text .. ' version2' end)",
        );
        assert!(host.reload_if_changed());
        assert_eq!(process(&mut host, "x"), "X version2");

        fs::remove_file(dir.0.join("tag.lua")).expect("plugin file should be removed");
        assert!(host.reload_if_changed());
        assert_eq!(process(&mut host, "x"), "X");
    }

    #[test]
    fn reload_keeps_plugins_before_the_first_change_and_reruns_the_rest_in_order() {
        let counter = "local n = 0
             dictate.on('transcript', function(text) n = n + 1; return text .. ' a' .. n end)";
        let dir = PluginDir::new(&[
            ("a.lua", counter),
            (
                "b.lua",
                "dictate.on('transcript', function(text) return text .. ' v1' end)",
            ),
        ]);
        let mut host = dir.host();
        assert_eq!(process(&mut host, "x"), "X a1 v1");

        // Only b reloads: a keeps its state.
        dir.write(
            "b.lua",
            "dictate.on('transcript', function(text) return text .. ' version2' end)",
        );
        assert!(host.reload_if_changed());
        assert_eq!(process(&mut host, "x"), "X a2 version2");

        // a reloads, and b after it, still in file order.
        dir.write("a.lua", &format!("{counter}\n-- edited"));
        assert!(host.reload_if_changed());
        assert_eq!(process(&mut host, "x"), "X a1 version2");
    }

    #[test]
    fn a_file_that_fails_keeps_the_last_version_and_is_not_rerun_until_edited() {
        let dir = PluginDir::new(&[
            (
                "a.lua",
                "dictate.on('transcript', function(text) return text .. ' tries=' .. tostring(TRIES) end)",
            ),
            (
                "b.lua",
                "dictate.on('transcript', function(text) return text .. ' v1' end)",
            ),
        ]);
        let mut host = dir.host();
        assert_eq!(process(&mut host, "x"), "X tries=nil v1");

        dir.write(
            "b.lua",
            "TRIES = (TRIES or 0) + 1
             dictate.on('transcript', function(text) return text .. ' broken' end)
             error('broken on purpose')",
        );
        assert!(host.reload_if_changed());
        assert_eq!(process(&mut host, "x"), "X tries=1 v1");
        let b = status(&host, "b");
        assert!(b.active, "the last version keeps running");
        assert!(b.errors[0].contains("broken on purpose"), "{:?}", b.errors);

        // Editing a reloads b too, but b's broken file is not run again.
        dir.write(
            "a.lua",
            "dictate.on('transcript', function(text) return text .. ' tries:' .. tostring(TRIES) end)",
        );
        assert!(host.reload_if_changed());
        assert_eq!(process(&mut host, "x"), "X tries:1 v1");
    }

    #[cfg(unix)]
    #[test]
    fn a_chain_in_progress_skips_hooks_of_plugins_unloaded_meanwhile() {
        let dir = PluginDir::new(&[
            (
                "a.lua",
                "dictate.on('transcript', function(text) dictate.exec({ 'sleep', '0.2' }) return text .. ' a' end)",
            ),
            (
                "b.lua",
                "dictate.on('transcript', function(text) return text .. ' b' end)",
            ),
        ]);
        let mut host = dir.host();
        host.start_chain(1, &RawTranscript::new("x"), &TranscriptContext::default());

        fs::remove_file(dir.0.join("b.lua")).expect("plugin file should be removed");
        assert!(host.reload_if_changed());
        let done = host.inbox.recv().expect("a's command should finish");
        host.finish_exec(done.task, done.outcome);

        assert_eq!(host.finished.len(), 1);
        assert_eq!(host.finished[0].1.as_str(), "X a");
    }

    #[test]
    fn plugins_disabled_in_config_do_not_load() {
        let dir = PluginDir::new(&[(
            "shout.lua",
            "dictate.on('transcript', function(text) return text:upper() end)",
        )]);
        let mut setup = setup(Some(dir.0.clone()));
        setup.disabled = BTreeSet::from(["formatter".to_owned(), "shout".to_owned()]);
        let mut host = load(setup);

        assert_eq!(process(&mut host, "um hello period"), "um hello period");
        let shout = status(&host, "shout");
        assert!(!shout.active && shout.errors.is_empty(), "{shout:?}");
    }

    #[cfg(unix)]
    #[test]
    fn unloading_a_plugin_cancels_its_sessions_and_kills_its_commands() {
        let pid_file = std::env::temp_dir().join(format!(
            "dictate-unload-{}-{}.pid",
            std::process::id(),
            TEST_DIR_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let code = format!(
            "dictate.on('hotkey', function(event)
               if event.phase == 'down' then
                 SESSION = dictate.session.open()
               else
                 dictate.exec({{ 'sh', '-c', 'echo $$ > {}; exec sleep 30' }}, {{ timeout_ms = 60000 }})
               end
             end)",
            pid_file.display()
        );
        let dir = PluginDir::new(&[("rec.lua", &code)]);
        let mut host = dir.host();
        let press = |phase| PluginEvent::Hotkey {
            name: "other".to_owned(),
            phase,
            focus: None,
        };

        host.dispatch(&press(HotkeyPhase::Down));
        let calls = host.take_calls();
        assert_eq!(calls.len(), 1);
        host.reply(
            calls[0].0,
            Ok(CoreReply::Session {
                session: SessionId(5),
            }),
        );
        host.dispatch(&press(HotkeyPhase::Up));
        let waited = Instant::now();
        while !pid_file.exists() && waited.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = fs::read_to_string(&pid_file).expect("the command should start");
        drop(fs::remove_file(&pid_file));

        dir.write("rec.lua", &format!("{code}\n-- edited"));
        assert!(host.reload_if_changed());
        let calls = host
            .take_calls()
            .into_iter()
            .map(|(_, call)| call)
            .collect::<Vec<_>>();
        assert_eq!(
            calls,
            vec![CoreCall::SessionCancel {
                session: SessionId(5)
            }]
        );
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        let waited = Instant::now();
        while alive() && waited.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(), "the unloaded plugin's command should be killed");
    }

    #[test]
    fn a_session_opened_for_a_plugin_unloaded_meanwhile_is_cancelled() {
        let code = "dictate.on('hotkey', function() dictate.session.open() end)";
        let dir = PluginDir::new(&[("rec.lua", code)]);
        let mut host = dir.host();
        host.dispatch(&PluginEvent::Hotkey {
            name: "other".to_owned(),
            phase: HotkeyPhase::Down,
            focus: None,
        });
        let calls = host.take_calls();

        dir.write("rec.lua", &format!("{code}\n-- edited"));
        assert!(host.reload_if_changed());
        host.reply(
            calls[0].0,
            Ok(CoreReply::Session {
                session: SessionId(8),
            }),
        );
        let calls = host
            .take_calls()
            .into_iter()
            .map(|(_, call)| call)
            .collect::<Vec<_>>();
        assert_eq!(
            calls,
            vec![CoreCall::SessionCancel {
                session: SessionId(8)
            }]
        );
    }

    #[test]
    fn core_calls_fail_on_the_command_line() {
        let dir = PluginDir::new(&[(
            "focus.lua",
            "dictate.on('transcript', function(text)
               local ok, why = pcall(dictate.desktop.focus)
               return text .. ' ' .. tostring(ok) .. ' ' .. tostring(why):match('running daemon')
             end)",
        )]);
        let mut host = dir.host();

        assert_eq!(process(&mut host, "hi"), "Hi false running daemon");
    }
}
