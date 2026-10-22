//! Lua plugin host for the dictation pipeline.
//!
//! Plugins are trusted Lua 5.4 files. The host only guarantees its own
//! survival: every plugin call runs under a time budget, errors come back as
//! values, and a failing step keeps the text it was given, so the local
//! transcript is always the fallback.
//!
//! Plugins extend Dictate in two ways:
//!
//! - **Registries** (`dictate.dictionary`, `dictate.replacements`) are
//!   rebuilt, never mutated: each rebuild starts from the configured value
//!   and runs every registered `transform` once, in plugin order. Each
//!   transform gets a fresh copy, and its result must deserialize back into
//!   the registry's Rust type or that transform is skipped.
//! - **Hooks** (`dictate.on("transcript", fn)`) form an ordered
//!   text-in, text-out chain run on every utterance.
//!
//! The formatter is itself a bundled plugin on the same API.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::SystemTime;

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
mod process;
mod runtime;

pub use core::CoreCall;
pub use core::CoreReply;
pub use core::CoreResult;
pub use core::HotkeyPhase;
pub use core::PluginEvent;
pub use core::SessionId;

pub use api::lua_types;
pub use process::BlockingPlugins;
pub use process::CallId;
pub use process::HostCommand;
pub use process::HostEvent;
pub use process::HostId;
pub use process::HostMessage;
pub use process::Notify;
pub use process::PluginProcess;
pub use process::serve;

/// Time a plugin file gets to run its top-level code.
const LOAD_BUDGET: Duration = Duration::from_secs(1);
/// Time one transform call, or a hook without a `timeout_ms`, gets before it
/// is stopped.
const CALL_BUDGET: Duration = Duration::from_millis(250);
/// The longest `timeout_ms` a hook may ask for.
const MAX_HOOK_BUDGET: Duration = Duration::from_mins(2);

const BUNDLED: [(&str, &str); 3] = [
    ("formatter", include_str!("../plugins/formatter.lua")),
    ("agent", include_str!("../plugins/agent.lua")),
    ("dictation", include_str!("../plugins/dictation.lua")),
];

const HOOKS: &str = "dictate.hooks";
const HOTKEY_HANDLERS: &str = "dictate.events.hotkey";
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
/// The daemon sends it to the plugin host when the host starts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginSetup {
    pub mode: DictationMode,
    pub spoken_formatting: Option<SpokenFormatting>,
    pub dictionary: Vec<Entry>,
    pub replacements: Vec<Entry>,
    pub plugins_dir: Option<PathBuf>,
    /// Each plugin's `[plugins.<id>]` table from `config.toml`, passed to the
    /// plugin file as its argument (`local options = ...`).
    #[serde(default)]
    pub options: BTreeMap<String, serde_json::Value>,
}

/// What the core knows about where a transcript is going, for hooks.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TranscriptContext {
    /// The focused application's identifier when recording stopped, if
    /// known. Only captured for insert delivery.
    pub app_id: Option<String>,
    pub window_title: Option<String>,
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
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginStatus {
    pub id: String,
    /// `bundled`, or the plugin file's path.
    pub source: String,
    pub errors: Vec<String>,
}

/// Plugins loaded into one Lua VM. It runs inside the plugin-host process;
/// the daemon talks to it through [`PluginProcess`].
///
/// One thread runs all Lua. Work that waits (a command, later a core call)
/// parks its coroutine and comes back as a [`Message`], so a transcript hook
/// waiting on an agent never holds up other plugin work.
pub(crate) struct PluginHost {
    lua: Lua,
    setup: PluginSetup,
    plugins: Vec<PluginStatus>,
    inbox_tx: mpsc::Sender<Message>,
    inbox: mpsc::Receiver<Message>,
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
}

/// What the host's thread is woken by.
pub(crate) enum Message {
    /// A request from the daemon.
    Request(process::Request),
    /// A command a parked task was waiting on ended.
    Exec {
        task: u64,
        outcome: Result<api::ExecResult, String>,
    },
    /// The daemon closed the connection.
    Closed,
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
    budget: Duration,
}

/// Registrations are only accepted while a plugin file loads.
struct Loading(Cell<bool>);
/// The context `dictate.text.format` formats with.
struct EffectiveContext(RefCell<DictationContext>);

impl PluginHost {
    /// Load the bundled plugins and every `*.lua` file in the plugins
    /// directory, then rebuild the registries.
    ///
    /// A plugin that fails to load is reported in [`Self::plugins`] and
    /// leaves no registrations behind; only a failure to set up the Lua VM
    /// itself is an error.
    fn load(setup: PluginSetup, reporter: Option<exec::Reporter>) -> mlua::Result<Self> {
        let lua = Lua::new();
        if let Some(reporter) = reporter {
            exec::set_reporter(&lua, reporter);
        }
        install_api(&lua, setup.base_context())?;

        let plugins = plugin_sources(&fingerprint(setup.plugins_dir.as_deref()))
            .into_iter()
            .map(|source| {
                let options = setup.options.get(&source.id);
                load_plugin(&lua, source, options)
            })
            .collect();

        let (inbox_tx, inbox) = mpsc::channel();
        let mut host = Self {
            lua,
            setup,
            plugins,
            inbox_tx,
            inbox,
            next_task: 1,
            parked: HashMap::new(),
            next_chain: 1,
            chains: HashMap::new(),
            finished: Vec::new(),
            calls: Vec::new(),
        };
        host.rebuild_registries()?;
        Ok(host)
    }

    fn plugins(&self) -> &[PluginStatus] {
        &self.plugins
    }

    /// The longest one run of the hook chain may take: every hook's budget.
    fn chain_budget(&self) -> Duration {
        registrations(&self.lua, HOOKS)
            .map(|hooks| hooks.iter().map(|hook| hook.budget).sum())
            .unwrap_or_default()
    }

    /// Where messages for this host go: requests from the daemon's reader
    /// thread, and finished commands.
    fn sender(&self) -> mpsc::Sender<Message> {
        self.inbox_tx.clone()
    }

    /// Wait for the next message.
    fn next_message(&self) -> Message {
        // The host holds a sender itself, so the channel never disconnects.
        self.inbox.recv().unwrap_or(Message::Closed)
    }

    /// Chains finished since the last call, by request id.
    fn take_finished(&mut self) -> Vec<(u64, ProcessedDictation)> {
        std::mem::take(&mut self.finished)
    }

    /// Daemon calls plugin code made since the last call, by call id.
    fn take_calls(&mut self) -> Vec<(u64, core::CoreCall)> {
        std::mem::take(&mut self.calls)
    }

    /// Run every handler for `event`, each in its own coroutine.
    fn dispatch(&mut self, event: &core::PluginEvent) {
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
        };
        let (handlers, argument) = match (registrations(&self.lua, key), argument) {
            (Ok(handlers), Ok(argument)) => (handlers, argument),
            (Err(error), _) | (_, Err(error)) => {
                eprintln!("plugin event handlers are unavailable: {error}");
                return;
            }
        };
        for handler in handlers {
            match runtime::Task::new(
                &self.lua,
                &handler.plugin,
                handler.budget,
                &handler.function,
            ) {
                Ok(mut task) => {
                    let step = task.resume(&self.lua, argument.clone());
                    self.on_step(task, Owner::Event, step);
                }
                Err(error) => self.complete(Owner::Event, &handler.plugin, Err(error)),
            }
        }
    }

    /// The daemon answered call `call`; resume the task waiting on it.
    fn finish_call(&mut self, call: u64, result: core::CoreResult) {
        let Some((mut task, owner)) = self.parked.remove(&call) else {
            return;
        };
        let step = match self.lua.create_userdata(runtime::CoreReplyValue(result)) {
            Ok(reply) => task.resume(&self.lua, reply),
            Err(error) => runtime::Step::Failed(error),
        };
        self.on_step(task, owner, step);
    }

    /// Run the transcript hook chain to completion on this thread. For
    /// tests and in-process use; the plugin-host process uses
    /// [`Self::start_chain`] and its message loop.
    #[cfg(test)]
    fn process(
        &mut self,
        raw: &RawTranscript,
        transcript: &TranscriptContext,
    ) -> ProcessedDictation {
        const LOCAL: u64 = u64::MAX;
        self.start_chain(LOCAL, raw, transcript);
        loop {
            if let Some(index) = self.finished.iter().position(|(id, _)| *id == LOCAL) {
                return self.finished.remove(index).1;
            }
            match self.next_message() {
                Message::Exec { task, outcome } => self.finish_exec(task, outcome),
                Message::Request(_) | Message::Closed => {}
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
        let Some(chain) = self.chains.get_mut(&id) else {
            return;
        };
        let Some(hook) = chain.hooks.pop_front() else {
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
                let mut task =
                    runtime::Task::new(&self.lua, &hook.plugin, hook.budget, &hook.function)?;
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
            runtime::Call::Core(op) => self.calls.push((task_id, op)),
            runtime::Call::Chain { text, context } => {
                self.run_chain(ChainReply::Task(task_id), &text, &context);
            }
            runtime::Call::Exec(request) => {
                let reporter = exec::reporter(&self.lua);
                let inbox = self.sender();
                thread::spawn(move || {
                    let outcome = exec::run(&request, reporter.as_deref());
                    drop(inbox.send(Message::Exec {
                        task: task_id,
                        outcome,
                    }));
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

    fn rebuild_registries(&mut self) -> mlua::Result<()> {
        let dictionary = self.rebuild_registry(DICTIONARY, &self.setup.dictionary.clone())?;
        let replacements = self.rebuild_registry(REPLACEMENTS, &self.setup.replacements.clone())?;
        let context = dictation_context(
            self.setup.mode,
            self.setup.spoken_formatting,
            &dictionary,
            &replacements,
        );
        if let Some(effective) = self.lua.app_data_ref::<EffectiveContext>() {
            *effective.0.borrow_mut() = context;
        }
        Ok(())
    }

    fn rebuild_registry(&mut self, name: &str, initial: &[Entry]) -> mlua::Result<Vec<Entry>> {
        let mut current = initial.to_vec();
        for Registration {
            plugin,
            function: transform,
            budget,
        } in registrations(&self.lua, &transforms_key(name))?
        {
            let result = self.lua.to_value(&current).and_then(|value| {
                let returned =
                    call_plugin::<Value>(&self.lua, &plugin, budget, &transform, value.clone())?;
                let rebuilt = if returned.is_nil() { value } else { returned };
                self.lua.from_value::<Vec<Entry>>(rebuilt)
            });
            match result {
                Ok(rebuilt) => current = rebuilt,
                Err(error) => self.report(&plugin, format!("{name} transform skipped: {error}")),
            }
        }
        Ok(current)
    }

    fn report(&mut self, plugin: &str, error: String) {
        eprintln!("plugin {plugin}: {error}");
        if let Some(status) = self.plugins.iter_mut().find(|status| status.id == plugin) {
            status.errors.push(error);
        }
    }
}

struct PluginSource {
    id: String,
    source: String,
    code: Result<String, String>,
}

/// Bundled plugins first, in bundled order; a user file with a bundled
/// plugin's name takes that plugin's place. Remaining user files follow in
/// file-name order.
fn plugin_sources(files: &[(PathBuf, Option<SystemTime>, u64)]) -> Vec<PluginSource> {
    let mut user = files
        .iter()
        .map(|(path, ..)| {
            let id = path
                .file_stem()
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
            let code = fs::read_to_string(path).map_err(|error| format!("read failed: {error}"));
            PluginSource {
                id,
                source: path.display().to_string(),
                code,
            }
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
                code: Ok(code.to_owned()),
            });
        }
    }
    sources.extend(user);
    sources
}

fn load_plugin(
    lua: &Lua,
    source: PluginSource,
    options: Option<&serde_json::Value>,
) -> PluginStatus {
    let mut errors = Vec::new();
    match source.code {
        Ok(code) => {
            set_loading(lua, true);
            let result = lua
                .load(code)
                .set_name(format!("@{}", source.source))
                .into_function()
                .and_then(|chunk| {
                    let options = match options {
                        Some(options) => lua.to_value_with(options, json_to_lua())?,
                        None => Value::Nil,
                    };
                    call_plugin::<()>(lua, &source.id, LOAD_BUDGET, &chunk, options)
                });
            set_loading(lua, false);
            if let Err(error) = result {
                // Drop whatever the plugin registered before it failed, so a
                // half-loaded plugin never runs.
                if let Err(cleanup) = drop_registrations(lua, &source.id) {
                    errors.push(format!("cleanup failed: {cleanup}"));
                }
                errors.push(format!("load failed: {error}"));
            }
        }
        Err(error) => errors.push(error),
    }
    for error in &errors {
        eprintln!("plugin {}: {error}", source.id);
    }
    PluginStatus {
        id: source.id,
        source: source.source,
        errors,
    }
}

fn install_api(lua: &Lua, context: DictationContext) -> mlua::Result<()> {
    runtime::install(lua);
    lua.set_app_data(Loading(Cell::new(false)));
    lua.set_app_data(EffectiveContext(RefCell::new(context)));

    lua.set_named_registry_value(HOOKS, lua.create_table()?)?;
    lua.set_named_registry_value(HOTKEY_HANDLERS, lua.create_table()?)?;
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

fn hook_budget(timeout_ms: Option<u64>) -> mlua::Result<Duration> {
    let Some(timeout_ms) = timeout_ms else {
        return Ok(CALL_BUDGET);
    };
    let budget = Duration::from_millis(timeout_ms);
    if budget.is_zero() || budget > MAX_HOOK_BUDGET {
        return Err(mlua::Error::runtime(format!(
            "timeout_ms must be between 1 and {}",
            MAX_HOOK_BUDGET.as_millis()
        )));
    }
    Ok(budget)
}

fn register(lua: &Lua, key: &str, function: Function, budget: Duration) -> mlua::Result<()> {
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
    entry.set(
        "budget_ms",
        u64::try_from(budget.as_millis()).unwrap_or(u64::MAX),
    )?;
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
                budget: Duration::from_millis(entry.get("budget_ms")?),
            })
        })
        .collect()
}

fn drop_registrations(lua: &Lua, plugin: &str) -> mlua::Result<()> {
    let keys = [HOOKS.to_owned(), HOTKEY_HANDLERS.to_owned()]
        .into_iter()
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

/// Run plugin code to completion with `plugin` marked active and `budget`
/// of Lua time; core calls it makes are resolved on this thread.
fn call_plugin<R: FromLuaMulti>(
    lua: &Lua,
    plugin: &str,
    budget: Duration,
    function: &Function,
    args: impl IntoLuaMulti,
) -> mlua::Result<R> {
    let values = runtime::Task::new(lua, plugin, budget, function)?.run_blocking(lua, args)?;
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

/// The `*.lua` files in `dir`, sorted, with what identifies each version.
fn fingerprint(dir: Option<&Path>) -> Vec<(PathBuf, Option<SystemTime>, u64)> {
    let Some(entries) = dir.and_then(|dir| fs::read_dir(dir).ok()) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "lua"))
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            metadata
                .is_file()
                .then(|| (path, metadata.modified().ok(), metadata.len()))
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
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

        fn host(&self) -> PluginHost {
            PluginHost::load(setup(Some(self.0.clone())), None).expect("plugin host should load")
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
        }
    }

    fn process(host: &mut PluginHost, raw: &str) -> String {
        host.process(&RawTranscript::new(raw), &TranscriptContext::default())
            .as_str()
            .to_owned()
    }

    fn errors(host: &PluginHost, id: &str) -> Vec<String> {
        host.plugins()
            .iter()
            .find(|status| status.id == id)
            .map(|status| status.errors.clone())
            .unwrap_or_default()
    }

    #[test]
    fn bundled_formatter_formats_with_the_configured_dictionary() {
        let mut host = PluginHost::load(setup(None), None).expect("plugin host should load");

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
    fn a_runaway_hook_is_stopped_and_the_chain_continues() {
        let dir = PluginDir::new(&[
            (
                "loop.lua",
                "dictate.on('transcript', function() while true do end end)",
            ),
            (
                "after.lua",
                "dictate.on('transcript', function(text) return text .. '!' end)",
            ),
        ]);
        let mut host = dir.host();
        let started = Instant::now();

        assert_eq!(process(&mut host, "done period"), "Done.!");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_runaway_hook_that_catches_errors_is_still_stopped() {
        let dir = PluginDir::new(&[
            (
                "stubborn.lua",
                "dictate.on('transcript', function()
                   while true do pcall(function() while true do end end) end
                 end)",
            ),
            (
                "after.lua",
                "dictate.on('transcript', function(text) return text .. '!' end)",
            ),
        ]);
        let mut host = dir.host();
        let started = Instant::now();

        assert_eq!(process(&mut host, "done period"), "Done.!");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_runaway_plugin_file_is_stopped_at_load() {
        let dir = PluginDir::new(&[(
            "hang.lua",
            "dictate.on('transcript', function() return 'hijacked' end)
             while true do pcall(function() while true do end end) end",
        )]);
        let started = Instant::now();
        let mut host = dir.host();

        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            errors(&host, "hang")[0].contains("time budget"),
            "{:?}",
            errors(&host, "hang")
        );
        assert_eq!(process(&mut host, "hello period"), "Hello.");
    }

    #[test]
    fn transforms_get_fresh_copies_and_bad_results_are_skipped() {
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
                "dictate.dictionary.transform(function() return { { spoken = 'x' } } end)",
            ),
        ]);
        let mut host = dir.host();

        assert_eq!(
            process(&mut host, "gee pee you eye and rust lang"),
            "GPUI and Rust"
        );
        let bad = errors(&host, "b-bad");
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(bad[0].contains("dictionary transform skipped"), "{bad:?}");
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

    #[test]
    fn a_hook_can_ask_for_a_longer_budget() {
        let busy = |name: &str, options: &str| {
            format!(
                // Wall-clock wait: 1 to 2 s, far past the default budget.
                "dictate.on('transcript', function(text)
                   local started = os.time()
                   while os.time() - started < 2 do end
                   return text .. ' {name}'
                 end{options})"
            )
        };
        let dir = PluginDir::new(&[
            ("a-default.lua", &busy("default", "")),
            ("b-patient.lua", &busy("patient", ", { timeout_ms = 5000 }")),
            ("c-invalid.lua", &busy("invalid", ", { timeout_ms = 0 }")),
        ]);
        let mut host = dir.host();

        assert_eq!(process(&mut host, "hi"), "Hi patient");
        assert!(errors(&host, "c-invalid")[0].contains("timeout_ms"));
        assert_eq!(
            host.chain_budget(),
            CALL_BUDGET * 2 + Duration::from_secs(5)
        );
    }

    #[cfg(unix)]
    #[test]
    fn exec_returns_output_and_its_time_is_not_the_hooks() {
        let dir = PluginDir::new(&[(
            "shell.lua",
            "dictate.on('transcript', function(text)
               local echoed = assert(dictate.exec({ 'sh', '-c', 'cat; echo err >&2; exit 3' }, { stdin = text }))
               -- Longer than the hook's 250 ms budget; waiting is not the hook's time.
               local slept = assert(dictate.exec({ 'sleep', '0.6' }))
               -- Enough work afterwards that the budget is checked again.
               local sum = 0
               for i = 1, 100000 do sum = sum + i end
               local missing, why = dictate.exec({ 'no-such-program-for-dictate' })
               return table.concat({
                 echoed.stdout, echoed.stderr, tostring(echoed.code), tostring(echoed.timed_out),
                 tostring(slept.code), tostring(missing), why:match('could not run') or why,
               }, '|')
             end)",
        )]);
        let mut host = dir.host();

        assert_eq!(
            process(&mut host, "hi"),
            "Hi|err\n|3|false|0|nil|could not run"
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
        let finished = host.take_finished();
        assert_eq!(finished.len(), 1, "only the fast chain should be done");
        assert_eq!((finished[0].0, finished[0].1.as_str()), (2, "Fast done"));
        assert!(started.elapsed() < Duration::from_millis(400));

        let Message::Exec { task, outcome } = host.next_message() else {
            panic!("the slow chain should be woken by its command");
        };
        host.finish_exec(task, outcome);
        let finished = host.take_finished();
        assert_eq!(finished.len(), 1);
        assert_eq!((finished[0].0, finished[0].1.as_str()), (1, "Slow done"));
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
        let mut host = PluginHost::load(setup, None).expect("plugin host should load");
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
        let mut host = PluginHost::load(setup(None), None).expect("plugin host should load");

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

    // The process tests run this test binary as the plugin host.
    const CHILD: &str = "DICTATE_PLUGINS_TEST_CHILD";

    #[test]
    fn plugin_host_child() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        serve().expect("plugin host should serve");
    }

    impl PluginDir {
        fn command() -> HostCommand {
            Box::new(|| {
                let mut command = std::process::Command::new(std::env::current_exe()?);
                command
                    .args(["--exact", "tests::plugin_host_child", "--nocapture"])
                    .env(CHILD, "1");
                Ok(command)
            })
        }

        fn process(&self) -> BlockingPlugins {
            BlockingPlugins::new(Self::command(), setup(Some(self.0.clone())))
        }
    }

    fn run(process: &mut BlockingPlugins, raw: &str) -> String {
        process
            .process(&RawTranscript::new(raw), &TranscriptContext::default())
            .as_str()
            .to_owned()
    }

    /// Run a plugin host the way the daemon does, answering core calls with
    /// `core`, until `done` holds or five seconds pass. `core` also returns
    /// events to send right after its reply.
    fn drive(
        dir: &PluginDir,
        events: &[PluginEvent],
        mut core: impl FnMut(&CoreCall) -> (CoreResult, Vec<PluginEvent>),
        done: impl Fn() -> bool,
    ) -> PluginProcess {
        let (inbox_tx, inbox) = std::sync::mpsc::channel();
        let mut process = PluginProcess::new(
            PluginDir::command(),
            setup(Some(dir.0.clone())),
            std::sync::Arc::new(move |message| drop(inbox_tx.send(message))),
        );
        while process.is_starting() {
            let message = inbox.recv().expect("the host should report its start");
            drop(process.receive(message));
        }
        assert!(process.is_running());
        let mut pending = Vec::new();
        for event in events {
            pending.extend(process.event(event.clone()));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() && Instant::now() < deadline {
            for event in std::mem::take(&mut pending) {
                if let HostEvent::Call { call, op } = event {
                    let (result, then) = core(&op);
                    pending.extend(process.reply(call, result));
                    for event in then {
                        pending.extend(process.event(event));
                    }
                }
            }
            if let Ok(message) = inbox.recv_timeout(Duration::from_millis(50)) {
                pending.extend(process.receive(message));
            }
        }
        process
    }

    /// Press and release the dictation hotkey against the bundled
    /// `dictation.lua` and a fake core; returns what was delivered and the
    /// calls made. With `quick`, the key goes up before the claim is
    /// answered.
    fn dictate_once(
        quick: bool,
        heard: &str,
    ) -> (
        Vec<(String, Option<dictate_desktop::FocusSnapshot>)>,
        Vec<CoreCall>,
    ) {
        let dir = PluginDir::new(&[]);
        let stop_focus =
            dictate_desktop::FocusSnapshot::Focused(dictate_desktop::FocusedWindow::niri(
                dictate_desktop::NiriInstanceId::new(7, 1),
                42,
                Some("dev.zed.Zed"),
                Some("notes.md"),
            ));
        let hotkey = |phase| PluginEvent::Hotkey {
            name: "dictate".to_owned(),
            phase,
            focus: None,
        };
        let delivered = std::rc::Rc::new(RefCell::new(Vec::new()));
        let seen = std::rc::Rc::clone(&delivered);
        let calls = std::rc::Rc::new(RefCell::new(Vec::new()));
        let made = std::rc::Rc::clone(&calls);
        let events = if quick {
            vec![hotkey(HotkeyPhase::Down), hotkey(HotkeyPhase::Up)]
        } else {
            vec![hotkey(HotkeyPhase::Down)]
        };

        drop(drive(
            &dir,
            &events,
            |call| {
                made.borrow_mut().push(call.clone());
                match call {
                    CoreCall::SessionOpen => (
                        Ok(CoreReply::Session {
                            session: SessionId(9),
                        }),
                        if quick {
                            Vec::new()
                        } else {
                            vec![hotkey(HotkeyPhase::Up)]
                        },
                    ),
                    CoreCall::SessionStop { session } => {
                        assert_eq!(*session, SessionId(9));
                        let transcript = CoreReply::Transcript {
                            text: heard.to_owned(),
                            focus: stop_focus.clone(),
                        };
                        (Ok(transcript), Vec::new())
                    }
                    CoreCall::DesktopDeliver {
                        text,
                        target,
                        focus,
                    } => {
                        assert_eq!(*target, None, "the configured target applies");
                        seen.borrow_mut().push((text.clone(), focus.clone()));
                        let delivered = CoreReply::Delivered {
                            target: dictate_desktop::DeliveryTarget::Insert,
                        };
                        (Ok(delivered), Vec::new())
                    }
                    CoreCall::OverlayHide => (Ok(CoreReply::Done), Vec::new()),
                    other @ (CoreCall::SessionCancel { .. } | CoreCall::DesktopFocus) => {
                        (Err(format!("unexpected call {other:?}")), Vec::new())
                    }
                }
            },
            || {
                calls.borrow().iter().any(|call| {
                    matches!(
                        call,
                        CoreCall::DesktopDeliver { .. } | CoreCall::OverlayHide
                    )
                })
            },
        ));
        let delivered = delivered.borrow().clone();
        let calls = calls.borrow().clone();
        (delivered, calls)
    }

    #[test]
    fn bundled_dictation_formats_and_delivers_to_the_stop_focus() {
        let stop_focus =
            dictate_desktop::FocusSnapshot::Focused(dictate_desktop::FocusedWindow::niri(
                dictate_desktop::NiriInstanceId::new(7, 1),
                42,
                Some("dev.zed.Zed"),
                Some("notes.md"),
            ));
        for quick in [false, true] {
            let (delivered, calls) =
                dictate_once(quick, "gee pee you eye works comma right period");
            assert_eq!(
                delivered,
                vec![("GPUI works, right.".to_owned(), Some(stop_focus.clone()))],
                "quick tap: {quick}"
            );
            assert_eq!(calls.len(), 3, "quick tap: {quick}: {calls:?}");
        }
    }

    #[test]
    fn bundled_dictation_hides_the_overlay_when_nothing_was_heard() {
        let (delivered, calls) = dictate_once(false, "");
        assert!(delivered.is_empty());
        assert_eq!(calls.last(), Some(&CoreCall::OverlayHide));
    }

    #[test]
    fn the_plugin_process_runs_the_chain_despite_stray_output_and_input() {
        let dir = PluginDir::new(&[(
            "noisy.lua",
            "print('printed')
             io.stdout:write('written to stdout without a newline')
             io.write('{\"type\":\"ready\",\"plugins\":[],\"chain_budget_ms\":0}')
             os.execute('echo from a child process')
             dictate.on('transcript', function(text)
               print('during a hook')
               io.stdout:write('{\"type\":\"processed\",\"id\":99,\"text\":\"spoof\"}')
               assert(io.read('l') == nil, 'stdin should be closed')
               -- A child's output without a newline, right before the reply.
               os.execute('printf partial')
               return text .. ' ok'
             end, { timeout_ms = 2000 })",
        )]);
        let mut process = dir.process();

        assert!(process.is_running());
        assert_eq!(
            process.plugins().len(),
            4,
            "formatter, agent, dictation, noisy"
        );
        assert_eq!(
            run(&mut process, "hello comma world period"),
            "Hello, world. ok"
        );
        assert_eq!(run(&mut process, "again period"), "Again. ok");
    }

    #[cfg(unix)]
    #[test]
    fn a_blocked_plugin_host_is_killed_and_the_text_is_formatted_locally() {
        let pid_file = std::env::temp_dir().join(format!(
            "dictate-plugins-blocked-{}-{}.pid",
            std::process::id(),
            TEST_DIR_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let code = format!(
            "dictate.on('transcript', function(text, ctx)
               if ctx.raw:find('block') then
                 os.execute('echo $$ > {}; exec sleep 10')
               end
               return text .. ' ok'
             end)",
            pid_file.display()
        );
        let dir = PluginDir::new(&[("block.lua", &code)]);
        let mut process = dir.process();
        let started = Instant::now();

        assert_eq!(
            run(&mut process, "block gee pee you eye period"),
            "Block GPUI."
        );
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
        assert!(!process.is_running());

        // The plugin's own child process dies with the host.
        let pid = fs::read_to_string(&pid_file).expect("hook should record its pid");
        drop(fs::remove_file(&pid_file));
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        let waited = Instant::now();
        while alive() && waited.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(), "the plugin's sleep outlived the plugin host");

        assert_eq!(run(&mut process, "next period"), "Next. ok");
        assert!(process.is_running());
    }

    #[cfg(unix)]
    #[test]
    fn the_daemon_waits_for_a_command_past_the_hook_budget() {
        // The chain budget is 2 × 250 ms plus 1 s of slack; the command runs
        // longer than that, so only the command's own timeout extends it.
        let dir = PluginDir::new(&[(
            "slow.lua",
            "dictate.on('transcript', function(text)
               local result = assert(dictate.exec({ 'sleep', '2' }, { timeout_ms = 5000 }))
               return text .. ' slept ' .. tostring(result.code)
             end)",
        )]);
        let mut process = dir.process();

        assert_eq!(run(&mut process, "hello period"), "Hello. slept 0");
        assert!(process.is_running());
    }

    #[test]
    fn a_crashed_plugin_host_is_replaced_on_the_next_dictation() {
        let dir = PluginDir::new(&[(
            "crash.lua",
            "dictate.on('transcript', function(text, ctx)
               if ctx.raw:find('crash') then os.exit(3) end
               return text .. ' ok'
             end)",
        )]);
        let mut process = dir.process();

        assert_eq!(run(&mut process, "crash period"), "Crash.");
        assert_eq!(run(&mut process, "fine period"), "Fine. ok");
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_reload_does_not_hold_up_the_caller() {
        let dir = PluginDir::new(&[(
            "tag.lua",
            "dictate.on('transcript', function(text) return text .. ' v1' end)",
        )]);
        let mut process = dir.process();
        dir.write(
            "tag.lua",
            "os.execute('sleep 1')
             dictate.on('transcript', function(text) return text .. ' v2' end)",
        );

        let started = Instant::now();
        assert!(process.reload_if_changed());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        assert!(process.is_running(), "the old host keeps serving meanwhile");
        // A transcript waits for the edit to take effect.
        assert_eq!(run(&mut process, "x"), "X v2");
    }

    #[test]
    fn reload_restarts_the_host_after_edits_and_deletes() {
        let dir = PluginDir::new(&[(
            "tag.lua",
            "dictate.on('transcript', function(text) return text .. ' v1' end)",
        )]);
        let mut process = dir.process();
        assert_eq!(run(&mut process, "x"), "X v1");
        assert!(!process.reload_if_changed());

        dir.write(
            "tag.lua",
            "dictate.on('transcript', function(text) return text .. ' version2' end)",
        );
        assert!(process.reload_if_changed());
        assert_eq!(run(&mut process, "x"), "X version2");

        fs::remove_file(dir.0.join("tag.lua")).expect("plugin file should be removed");
        assert!(process.reload_if_changed());
        assert_eq!(run(&mut process, "x"), "X");
    }
}
