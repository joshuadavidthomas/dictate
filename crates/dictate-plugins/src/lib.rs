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
#[cfg(test)]
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

pub use core::Audio;
pub use core::CaptureId;
pub use core::CaptureSource;
pub use core::CoreCall;
pub use core::CoreReply;
pub use core::CoreResult;
pub use core::HotkeyPhase;
pub use core::PluginEvent;
pub use core::SessionId;

pub use api::Feed;
pub use api::FeedGroup;
pub use api::FeedRow;
pub use api::PageQuery;
pub use api::lua_types;

const BUNDLED: [(&str, &str); 7] = [
    ("recognizer", include_str!("../plugins/recognizer.lua")),
    ("openai", include_str!("../plugins/openai.lua")),
    ("formatter", include_str!("../plugins/formatter.lua")),
    ("agent", include_str!("../plugins/agent.lua")),
    ("dictation", include_str!("../plugins/dictation.lua")),
    ("meeting", include_str!("../plugins/meeting.lua")),
    ("history", include_str!("../plugins/history.lua")),
];

const AUDIO_HOOKS: &str = "dictate.hooks.audio";
const TRANSCRIPT_HOOKS: &str = "dictate.hooks.transcript";
const DELIVER_HOOKS: &str = "dictate.hooks.deliver";
const HOOKS: [&str; 3] = [AUDIO_HOOKS, TRANSCRIPT_HOOKS, DELIVER_HOOKS];
const HOTKEY_HANDLERS: &str = "dictate.events.hotkey";
const RECORD_HANDLERS: &str = "dictate.events.record";
const SESSION_HANDLERS: &str = "dictate.events.session";
const EVENT_HANDLERS: [&str; 3] = [HOTKEY_HANDLERS, RECORD_HANDLERS, SESSION_HANDLERS];
/// Pages for the window, from `dictate.ui.page`; each entry also has `id`
/// and `title`.
const PAGES: &str = "dictate.ui.pages";
/// The name local tasks run under; not a valid plugin file stem.
const LOCAL: &str = "(dictate)";
const DICTIONARY: &str = "dictionary";
const REPLACEMENTS: &str = "replacements";
const REGISTRIES: [&str; 2] = [DICTIONARY, REPLACEMENTS];
/// The recognizers registry: entries hold functions, so it is a Lua table
/// rather than a list of [`Entry`].
const RECOGNIZERS: &str = "recognizers";

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
    /// Where `dictate.data_dir` makes each plugin's directory.
    pub data_dir: Option<PathBuf>,
    /// Each plugin's `[plugins.<id>]` table from `config.toml`, passed to the
    /// plugin file as its argument (`local options = ...`).
    pub options: BTreeMap<String, serde_json::Value>,
    /// Plugins with `enabled = false`; they are listed but never loaded.
    pub disabled: BTreeSet<String>,
    /// The id of the recognizer `dictate.transcribe` runs.
    pub recognizer: String,
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

/// A page a plugin added to the window with `dictate.ui.page`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginPage {
    pub id: String,
    pub title: String,
    /// The plugin that added it.
    pub plugin: String,
    /// Whether the window shows a search box above it.
    pub search: bool,
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
    /// Hook chains in progress.
    chains: HashMap<u64, Chain>,
    next_local: u64,
    /// Local tasks ([`Self::process`], [`Self::transcribe`]) that finished,
    /// by id: what they returned, or why they failed.
    finished: Vec<(u64, Result<MultiValue, String>)>,
    /// Event handlers that failed after receiving a transcript, since the
    /// last [`Self::take_failures`].
    failures: Vec<String>,
    /// Daemon calls made since the last [`Self::take_calls`].
    calls: Vec<(u64, core::CoreCall)>,
    /// Running commands' cancel flags, by the task waiting on them.
    execs: HashMap<u64, Arc<AtomicBool>>,
    /// Open recording sessions and captures, and the plugin that opened
    /// each.
    handles: HashMap<Handle, String>,
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
    /// One hook of a chain.
    Chain(u64),
    /// An event handler; only failures matter. `recording` is set once it
    /// received a recording, so its failure means a dictation was lost.
    Event { recording: bool },
    /// A task the host runs for the command line, by id.
    Local(u64),
}

/// A run of the hooks for one point, sharing one event table.
struct Chain {
    /// The task parked in `dictate.transcribe`, `dictate.transcript.run`,
    /// or `dictate.desktop.deliver`.
    reply: u64,
    /// `audio`, `transcript`, or `deliver`, for messages.
    point: &'static str,
    event: Table,
    hooks: VecDeque<Registration>,
}

/// A core resource a plugin holds, closed when the plugin goes away.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Handle {
    Session(core::SessionId),
    Capture(core::CaptureId),
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
/// Each registry's entries as of the last rebuild, by registry name.
struct RegistryEntries(RefCell<BTreeMap<&'static str, Vec<Entry>>>);
/// The id of the recognizer `dictate.transcribe` runs.
struct SelectedRecognizer(String);
/// [`PluginSetup::data_dir`].
struct DataDir(Option<PathBuf>);

impl Plugins {
    /// Load the bundled plugins and every `*.lua` file in the plugins
    /// directory, then rebuild the registries.
    ///
    /// A plugin that fails to load is reported in [`Self::plugins`] and
    /// leaves no registrations behind; only a failure to set up the Lua VM
    /// itself is an error.
    pub fn load(setup: PluginSetup, wake: Wake) -> mlua::Result<Self> {
        let lua = Lua::new();
        install_api(&lua, &setup)?;

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
            next_local: 1,
            finished: Vec::new(),
            failures: Vec::new(),
            calls: Vec::new(),
            execs: HashMap::new(),
            handles: HashMap::new(),
        };
        host.reload_if_changed();
        Ok(host)
    }

    /// The pages plugins added, in plugin order.
    #[must_use]
    pub fn pages(&self) -> Vec<PluginPage> {
        let pages = self
            .lua
            .named_registry_value::<Table>(PAGES)
            .and_then(|pages| {
                pages
                    .sequence_values::<Table>()
                    .map(|entry| {
                        let entry = entry?;
                        Ok(PluginPage {
                            id: entry.get("id")?,
                            title: entry.get("title")?,
                            plugin: entry.get("plugin")?,
                            search: entry.get::<Option<bool>>("search")?.unwrap_or(false),
                        })
                    })
                    .collect::<mlua::Result<Vec<_>>>()
            });
        pages.unwrap_or_else(|error| {
            eprintln!("plugin pages are unavailable: {error}");
            Vec::new()
        })
    }

    /// Run page `id`'s `render` for `query`.
    pub fn render_page(&mut self, id: &str, query: &PageQuery) -> Result<Feed, String> {
        let entry = self
            .lua
            .named_registry_value::<Table>(PAGES)
            .and_then(|pages| {
                for entry in pages.sequence_values::<Table>() {
                    let entry = entry?;
                    if entry.get::<String>("id")? == id {
                        return Ok(Some(entry));
                    }
                }
                Ok(None)
            })
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("no plugin has a page {id:?}"))?;
        let render = || -> mlua::Result<Feed> {
            let plugin: String = entry.get("plugin")?;
            let function: Function = entry.get("fn")?;
            let query = self.lua.to_value_with(query, json_to_lua())?;
            let feed: Value = call_plugin(&self.lua, &plugin, &function, query)?;
            self.lua.from_value(feed)
        };
        render().map_err(|error| error.to_string())
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
        self.reload_before(usize::MAX)
    }

    /// Take new configuration, such as after `config.toml` changed. The
    /// registries are rebuilt from it, and plugins are reloaded from the
    /// first one whose options or `enabled` changed, as for an edited file.
    pub fn reconfigure(&mut self, setup: PluginSetup) {
        let affected = |id: &str| {
            self.setup.options.get(id) != setup.options.get(id)
                || self.setup.disabled.contains(id) != setup.disabled.contains(id)
        };
        let first_affected = self
            .slots
            .iter()
            .position(|slot| affected(&slot.status.id))
            .unwrap_or(usize::MAX);
        self.lua
            .set_app_data(SelectedRecognizer(setup.recognizer.clone()));
        self.setup = setup;
        if !self.reload_before(first_affected) {
            self.rebuild_registries();
        }
    }

    /// Reload every plugin from the first that changed on disk, or from
    /// `limit` if that comes first. Whether anything reloaded.
    fn reload_before(&mut self, limit: usize) -> bool {
        let sources = plugin_sources(plugin_files(self.setup.plugins_dir.as_deref()));
        let unchanged = self
            .slots
            .iter()
            .zip(&sources)
            .take(limit)
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
            if let Owner::Chain(_) | Owner::Event { recording: true } = owner {
                self.complete(
                    owner,
                    plugin,
                    Err(mlua::Error::runtime("its plugin was unloaded")),
                );
            }
        }
        let handles = self
            .handles
            .iter()
            .filter(|(_, owner)| *owner == plugin)
            .map(|(&handle, _)| handle)
            .collect::<Vec<_>>();
        for handle in handles {
            self.handles.remove(&handle);
            self.release(handle);
        }
    }

    /// Close a handle whose plugin is gone; nobody waits for the answer.
    fn release(&mut self, handle: Handle) {
        let id = self.next_task;
        self.next_task += 1;
        self.calls.push((
            id,
            match handle {
                Handle::Session(session) => core::CoreCall::SessionCancel { session },
                Handle::Capture(capture) => core::CoreCall::CaptureClose { capture },
            },
        ));
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
                    let step = task.start(&self.lua, argument.clone());
                    self.on_step(task, Owner::Event { recording: false }, step);
                }
                Err(error) => self.complete(
                    Owner::Event { recording: false },
                    &handler.plugin,
                    Err(error),
                ),
            }
        }
    }

    /// Answer core call `call` and resume the task waiting on it. A reply
    /// for a task that is gone, for example after a reload, is dropped.
    pub fn reply(&mut self, call: CallId, result: core::CoreResult) {
        let opened = match &result {
            Ok(core::CoreReply::Session { session }) => Some(Handle::Session(*session)),
            Ok(core::CoreReply::Capture { capture }) => Some(Handle::Capture(*capture)),
            _ => None,
        };
        let got_recording = matches!(result, Ok(core::CoreReply::Recording { .. }));
        let Some((mut task, mut owner)) = self.parked.remove(&call.0) else {
            // Opened for a plugin that was unloaded while it waited.
            if let Some(handle) = opened {
                self.release(handle);
            }
            return;
        };
        if let Some(handle) = opened {
            self.handles.insert(handle, task.plugin().to_owned());
        }
        if let (true, Owner::Event { recording }) = (got_recording, &mut owner) {
            *recording = true;
        }
        let step = match self.lua.create_userdata(runtime::CoreReplyValue(result)) {
            Ok(reply) => task.resume(&self.lua, reply),
            Err(error) => runtime::Step::Failed(error),
        };
        self.on_step(task, owner, step);
    }

    /// Event handlers that failed after receiving a recording's transcript,
    /// since the last call: dictations that were lost, by plugin.
    pub fn take_failures(&mut self) -> Vec<String> {
        std::mem::take(&mut self.failures)
    }

    #[cfg(test)]
    /// Run the `transcript` hooks on `raw` to completion on this thread.
    /// Core calls fail: there is no daemon to serve them.
    ///
    /// # Errors
    ///
    /// The first hook that failed, and why.
    fn process(&mut self, raw: &RawTranscript) -> Result<ProcessedDictation, String> {
        self.process_for(raw, None)
    }

    #[cfg(test)]
    fn process_for(
        &mut self,
        raw: &RawTranscript,
        focus: Option<&api::Focus>,
    ) -> Result<ProcessedDictation, String> {
        let id = self.start_process(raw.as_str(), focus)?;
        let values =
            self.run_local(id, |_| Err("core calls need the running daemon".to_owned()))?;
        Ok(ProcessedDictation::new(local_string(&values, 0)?))
    }

    #[cfg(test)]
    /// Start the `transcript` hooks on `raw` as a local task; returns its id.
    fn start_process(&mut self, raw: &str, focus: Option<&api::Focus>) -> Result<u64, String> {
        let focus = self
            .lua
            .to_value_with(&focus, json_to_lua())
            .map_err(|error| error.to_string())?;
        self.start_local(
            "local text, focus = ...
             return dictate.transcript.run(text, { focus = focus })",
            (raw.to_owned(), focus),
        )
    }

    /// Transcribe `audio` the way the bundled dictation does, to completion
    /// on this thread, for the command line: the `audio` hooks and the
    /// selected recognizer, then the `transcript` hooks. `decode` serves
    /// `dictate.speech.decode`, or says why it cannot; other core calls
    /// fail.
    ///
    /// Returns the recognizer's text and the delivered text.
    ///
    /// # Errors
    ///
    /// The hook or recognizer that failed, and why.
    pub fn transcribe(
        &mut self,
        audio: Vec<f32>,
        mut decode: impl FnMut(Vec<f32>) -> Result<String, String>,
    ) -> Result<(String, ProcessedDictation), String> {
        let id = self.start_local(
            "local raw = dictate.transcribe(...)
             return raw, dictate.transcript.run(raw)",
            core::Audio(audio),
        )?;
        let values = self.run_local(id, |call| match call {
            core::CoreCall::Decode { audio } => {
                decode(audio.0).map(|text| core::CoreReply::Text { text })
            }
            core::CoreCall::SessionOpen
            | core::CoreCall::CaptureOpen { .. }
            | core::CoreCall::CaptureRead { .. }
            | core::CoreCall::CaptureClose { .. }
            | core::CoreCall::SessionStop { .. }
            | core::CoreCall::SessionCancel { .. }
            | core::CoreCall::DesktopFocus
            | core::CoreCall::DesktopDeliver { .. }
            | core::CoreCall::OverlayHide => Err("core calls need the running daemon".to_owned()),
        })?;
        Ok((
            local_string(&values, 0)?,
            ProcessedDictation::new(local_string(&values, 1)?),
        ))
    }

    /// Run `code` with `args` in a coroutine the host drives, as the core
    /// rather than a plugin. Its result lands in `self.finished`.
    fn start_local(&mut self, code: &str, args: impl IntoLuaMulti) -> Result<u64, String> {
        let id = self.next_local;
        self.next_local += 1;
        let started = self
            .lua
            .load(code)
            .set_name("=dictate")
            .into_function()
            .and_then(|function| runtime::Task::new(&self.lua, LOCAL, &function));
        let mut task = started.map_err(|error| error.to_string())?;
        let step = task.start(&self.lua, args);
        self.on_step(task, Owner::Local(id), step);
        Ok(id)
    }

    /// Drive local task `id` to its end, serving core calls with `serve`.
    fn run_local(
        &mut self,
        id: u64,
        mut serve: impl FnMut(core::CoreCall) -> core::CoreResult,
    ) -> Result<MultiValue, String> {
        loop {
            for (call, op) in self.take_calls() {
                let result = serve(op);
                self.reply(call, result);
            }
            if let Some(index) = self.finished.iter().position(|(done, _)| *done == id) {
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

    fn run_chain(&mut self, reply: u64, key: &'static str, event: Table) {
        let hooks = registrations(&self.lua, key).unwrap_or_else(|error| {
            eprintln!("{key} hooks are unavailable: {error}");
            Vec::new()
        });
        let id = self.next_chain;
        self.next_chain += 1;
        let point = match key {
            AUDIO_HOOKS => "audio",
            DELIVER_HOOKS => "deliver",
            _ => "transcript",
        };
        self.chains.insert(
            id,
            Chain {
                reply,
                point,
                event,
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
        let Some(hook) = next else {
            if let Some(chain) = self.chains.remove(&id) {
                self.finish_chain(chain.reply, Ok(chain.event));
            }
            return;
        };
        let Some(event) = self.chains.get(&id).map(|chain| chain.event.clone()) else {
            return;
        };
        let started =
            runtime::Task::new(&self.lua, &hook.plugin, &hook.function).map(|mut task| {
                let step = task.start(&self.lua, event);
                (task, step)
            });
        match started {
            Ok((task, step)) => self.on_step(task, Owner::Chain(id), step),
            Err(error) => self.complete(Owner::Chain(id), &hook.plugin, Err(error)),
        }
    }

    /// Hand a finished chain's event, or the failure that ended it, to
    /// whoever ran it.
    fn finish_chain(&mut self, reply: u64, result: Result<Table, String>) {
        let Some((mut task, owner)) = self.parked.remove(&reply) else {
            return;
        };
        let resumed = match result {
            Ok(event) => Ok(Value::Table(event)),
            Err(failure) => self
                .lua
                .create_userdata(runtime::CoreReplyValue(Err(failure)))
                .map(Value::UserData),
        };
        let step = match resumed {
            Ok(value) => task.resume(&self.lua, value),
            Err(error) => runtime::Step::Failed(error),
        };
        self.on_step(task, owner, step);
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
                    self.handles.remove(&Handle::Session(*session));
                } else if let core::CoreCall::CaptureClose { capture } = &op {
                    self.handles.remove(&Handle::Capture(*capture));
                }
                self.calls.push((task_id, op));
            }
            runtime::Call::Hooks { key, event } => {
                self.run_chain(task_id, key, event);
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
                let Some(point) = self.chains.get(&id).map(|chain| chain.point) else {
                    return;
                };
                let failure = match result {
                    Ok(values) => match values.into_iter().next() {
                        None | Some(Value::Nil) => None,
                        Some(other) => Some(format!(
                            "returned a {}; {point} hooks change the event's fields instead",
                            other.type_name()
                        )),
                    },
                    Err(error) => Some(error.to_string()),
                };
                match failure {
                    None => self.advance_chain(id),
                    Some(failure) => {
                        let failure = format!("plugin {plugin}: {point} hook failed: {failure}");
                        eprintln!("{failure}");
                        if let Some(chain) = self.chains.remove(&id) {
                            self.finish_chain(chain.reply, Err(failure));
                        }
                    }
                }
            }
            Owner::Event { recording } => {
                if let Err(error) = result {
                    let failure = format!("plugin {plugin}: event handler failed: {error}");
                    eprintln!("{failure}");
                    if recording {
                        self.failures.push(failure);
                    }
                }
            }
            Owner::Local(id) => self
                .finished
                .push((id, result.map_err(|error| error.to_string()))),
        }
    }

    /// Rebuild every registry. A plugin whose transform fails is disabled
    /// and the rebuild starts over without it.
    fn rebuild_registries(&mut self) {
        let (context, entries, recognizers) = loop {
            match self.try_rebuild() {
                Ok(rebuilt) => break rebuilt,
                Err((plugin, error)) => self.disable(&plugin, &error),
            }
        };
        if let Some(effective) = self.lua.app_data_ref::<EffectiveContext>() {
            *effective.0.borrow_mut() = context;
        }
        if let Some(current) = self.lua.app_data_ref::<RegistryEntries>() {
            *current.0.borrow_mut() = entries;
        }
        if let Err(error) = self.lua.set_named_registry_value(RECOGNIZERS, recognizers) {
            eprintln!("the recognizers registry could not be stored: {error}");
        }
    }

    #[allow(clippy::type_complexity)] // one caller destructures it at once
    fn try_rebuild(
        &self,
    ) -> Result<(DictationContext, BTreeMap<&'static str, Vec<Entry>>, Table), (String, String)>
    {
        let dictionary = self.rebuild_registry(DICTIONARY, &self.setup.dictionary)?;
        let replacements = self.rebuild_registry(REPLACEMENTS, &self.setup.replacements)?;
        let context = dictation_context(
            self.setup.mode,
            self.setup.spoken_formatting,
            &dictionary,
            &replacements,
        );
        let entries = BTreeMap::from([(DICTIONARY, dictionary), (REPLACEMENTS, replacements)]);
        Ok((context, entries, self.rebuild_recognizers()?))
    }

    /// The recognizers registry rebuilt from an empty list, or the plugin
    /// whose transform failed and why.
    fn rebuild_recognizers(&self) -> Result<Table, (String, String)> {
        let transforms =
            registrations(&self.lua, &transforms_key(RECOGNIZERS)).unwrap_or_else(|error| {
                eprintln!("recognizers transforms are unavailable: {error}");
                Vec::new()
            });
        let mut current = self
            .lua
            .create_table()
            .map_err(|error| (LOCAL.to_owned(), error.to_string()))?;
        for Registration { plugin, function } in transforms {
            let rebuilt = call_plugin::<Value>(&self.lua, &plugin, &function, current.clone())
                .and_then(|returned| {
                    if returned.is_nil() {
                        return Ok(current.clone());
                    }
                    returned.as_table().cloned().ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "returned a {}, not a list",
                            returned.type_name()
                        ))
                    })
                })
                .and_then(|list| {
                    for (index, entry) in list.clone().sequence_values::<Value>().enumerate() {
                        let valid = matches!(&entry?, Value::Table(entry)
                            if entry.get::<Value>("id")?.is_string()
                                && entry.get::<Value>("transcribe")?.is_function());
                        if !valid {
                            return Err(mlua::Error::runtime(format!(
                                "entry {} needs a string `id` and a `transcribe` function",
                                index + 1
                            )));
                        }
                    }
                    Ok(list)
                });
            match rebuilt {
                Ok(rebuilt) => current = rebuilt,
                Err(error) => {
                    return Err((plugin, format!("recognizers transform failed: {error}")));
                }
            }
        }
        Ok(current)
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

fn install_api(lua: &Lua, setup: &PluginSetup) -> mlua::Result<()> {
    runtime::install(lua);
    lua.set_app_data(Loading(Cell::new(false)));
    lua.set_app_data(EffectiveContext(RefCell::new(setup.base_context())));
    lua.set_app_data(RegistryEntries(RefCell::new(BTreeMap::from([
        (DICTIONARY, setup.dictionary.clone()),
        (REPLACEMENTS, setup.replacements.clone()),
    ]))));
    lua.set_app_data(SelectedRecognizer(setup.recognizer.clone()));
    lua.set_app_data(DataDir(setup.data_dir.clone()));
    lua.set_named_registry_value(RECOGNIZERS, lua.create_table()?)?;

    for key in HOOKS {
        lua.set_named_registry_value(key, lua.create_table()?)?;
    }
    for key in EVENT_HANDLERS.into_iter().chain([PAGES]) {
        lua.set_named_registry_value(key, lua.create_table()?)?;
    }
    for name in REGISTRIES.into_iter().chain([RECOGNIZERS]) {
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
    register_entry(lua, key, function).map(drop)
}

/// Register `function` under `key` for the loading plugin, and return the
/// entry so callers can add fields.
fn register_entry(lua: &Lua, key: &str, function: Function) -> mlua::Result<Table> {
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
    lua.named_registry_value::<Table>(key)?.push(&entry)?;
    Ok(entry)
}

fn register_page(
    lua: &Lua,
    (id, title, render, options): (String, String, Function, Option<Value>),
) -> mlua::Result<()> {
    let options: api::PageOptions = match options {
        Some(options) => lua.from_value(options)?,
        None => api::PageOptions::default(),
    };
    if id.is_empty() || title.is_empty() {
        return Err(mlua::Error::runtime(
            "dictate.ui.page: id and title must not be empty",
        ));
    }
    for entry in lua
        .named_registry_value::<Table>(PAGES)?
        .sequence_values::<Table>()
    {
        let entry = entry?;
        if entry.get::<String>("id")? == id {
            let plugin: String = entry.get("plugin")?;
            return Err(mlua::Error::runtime(format!(
                "dictate.ui.page: plugin {plugin:?} already added a page {id:?}"
            )));
        }
    }
    let entry = register_entry(lua, PAGES, render)?;
    entry.set("id", id)?;
    entry.set("search", options.search)?;
    entry.set("title", title)
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
    let keys = HOOKS
        .into_iter()
        .chain(EVENT_HANDLERS)
        .chain([PAGES])
        .map(str::to_owned)
        .chain(
            REGISTRIES
                .into_iter()
                .chain([RECOGNIZERS])
                .map(transforms_key),
        );
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

/// String result `index` of a local task.
fn local_string(values: &MultiValue, index: usize) -> Result<String, String> {
    match values.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        other => Err(format!(
            "expected a string, got {}",
            other.map_or("nothing", Value::type_name)
        )),
    }
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
            data_dir: None,
            options: BTreeMap::new(),
            disabled: BTreeSet::new(),
            recognizer: "local".to_owned(),
        }
    }

    fn load(setup: PluginSetup) -> Plugins {
        Plugins::load(setup, Arc::new(|| {})).expect("plugins should load")
    }

    fn process(host: &mut Plugins, raw: &str) -> String {
        host.process(&RawTranscript::new(raw))
            .expect("the transcript hooks should succeed")
            .as_str()
            .to_owned()
    }

    /// Start the transcript hooks on `raw` without waiting; returns the id
    /// its result in `host.finished` will have.
    fn start(host: &mut Plugins, raw: &str) -> u64 {
        host.start_process(raw, None)
            .expect("the task should start")
    }

    fn finished(host: &mut Plugins) -> Vec<(u64, String)> {
        std::mem::take(&mut host.finished)
            .into_iter()
            .map(|(id, values)| {
                let values = values.expect("the chain should succeed");
                (id, local_string(&values, 0).expect("a string"))
            })
            .collect()
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
    fn reconfigure_applies_new_entries_and_reloads_plugins_whose_options_changed() {
        let dir = PluginDir::new(&[(
            "suffix.lua",
            "local options = ... or {}\ndictate.on('transcript', function(e) e.text = e.text .. (options.suffix or '') end)",
        )]);
        let mut setup = setup(Some(dir.0.clone()));
        setup
            .options
            .insert("suffix".to_owned(), serde_json::json!({ "suffix": " (a)" }));
        let mut host = load(setup.clone());
        assert_eq!(process(&mut host, "bark works period"), "Bark works. (a)");

        setup.dictionary.push(Entry {
            spoken: "bark".to_owned(),
            written: "BARK".to_owned(),
        });
        host.reconfigure(setup.clone());
        assert_eq!(process(&mut host, "bark works period"), "BARK works. (a)");

        setup
            .options
            .insert("suffix".to_owned(), serde_json::json!({ "suffix": " (b)" }));
        host.reconfigure(setup.clone());
        assert_eq!(process(&mut host, "bark works period"), "BARK works. (b)");

        setup.disabled.insert("suffix".to_owned());
        host.reconfigure(setup);
        assert_eq!(process(&mut host, "bark works period"), "BARK works.");
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
    fn hooks_share_one_event_in_plugin_order_after_the_formatter() {
        let dir = PluginDir::new(&[
            (
                "b-shout.lua",
                "dictate.on('transcript', function(e) e.text = e.text:upper() end)",
            ),
            (
                "a-note.lua",
                "dictate.on('transcript', function(e) e.seen = e.text end)",
            ),
            (
                "c-mark.lua",
                "dictate.on('transcript', function(e)
                   e.text = e.text .. ' [' .. e.mode .. ' raw=' .. e.raw .. ' seen=' .. e.seen .. ']'
                 end)",
            ),
        ]);
        let mut host = dir.host();

        assert_eq!(
            process(&mut host, "hello comma world period"),
            "HELLO, WORLD. [message raw=hello comma world period seen=Hello, world.]"
        );
    }

    #[test]
    fn a_failing_hook_fails_the_transcript_and_later_hooks_do_not_run() {
        for (name, code, expected) in [
            (
                "a-fail.lua",
                "dictate.on('transcript', function() error('boom') end)",
                "plugin a-fail: transcript hook failed",
            ),
            (
                "a-old.lua",
                "dictate.on('transcript', function(e) return e.text .. '!' end)",
                "returned a string; transcript hooks change the event's fields instead",
            ),
        ] {
            let dir = PluginDir::new(&[
                (name, code),
                (
                    "b-after.lua",
                    "dictate.on('transcript', function() error('b should not run') end)",
                ),
            ]);
            let mut host = dir.host();

            let failure = host
                .process(&RawTranscript::new("hello period"))
                .expect_err("the transcript should fail");
            assert!(failure.contains(expected), "{failure}");
            assert!(!failure.contains("b should not run"), "{failure}");
        }

        let dir = PluginDir::new(&[(
            "a-number.lua",
            "dictate.on('transcript', function(e) e.text = 42 end)",
        )]);
        let failure = dir
            .host()
            .process(&RawTranscript::new("hi"))
            .expect_err("a non-string text should fail");
        assert!(failure.contains("event.text as a integer"), "{failure}");
    }

    /// Plugins in a directory, with `recognizer` selected.
    fn with_recognizer(dir: &PluginDir, recognizer: &str) -> Plugins {
        let mut setup = setup(Some(dir.0.clone()));
        setup.recognizer = recognizer.to_owned();
        load(setup)
    }

    #[test]
    fn audio_hooks_run_before_the_selected_recognizer() {
        let dir = PluginDir::new(&[
            (
                "trim.lua",
                "dictate.on('audio', function(e)
                   e.audio = e.audio:slice(2, #e.audio - 1)
                   e.audio[1] = e.audio[1] * 2
                 end)",
            ),
            (
                "echo.lua",
                "dictate.recognizers.transform(function(list)
                   table.insert(list, { id = 'echo', transcribe = function(a)
                     return ('  %d samples %.2f %.2f rate %d %s%s  '):format(
                       #a, a[1], a[2], a.rate, tostring(a[0]), tostring(a[#a + 1]))
                   end })
                 end)",
            ),
        ]);
        let audio = vec![0.1, 0.2, 0.3, 0.4];

        let (raw, _) = with_recognizer(&dir, "echo")
            .transcribe(audio.clone(), |_| panic!("echo does not use the core"))
            .expect("echo should transcribe");
        assert_eq!(raw, "2 samples 0.40 0.30 rate 16000 nilnil");

        let mut decoded = Vec::new();
        let (raw, text) = with_recognizer(&dir, "local")
            .transcribe(audio.clone(), |audio| {
                decoded = audio;
                Ok("hi period".to_owned())
            })
            .expect("the local recognizer should transcribe");
        assert_eq!((raw.as_str(), text.as_str()), ("hi period", "Hi."));
        assert_eq!(decoded, vec![0.4, 0.3]);

        let failure = with_recognizer(&dir, "nope")
            .transcribe(audio, |_| Ok(String::new()))
            .expect_err("an unknown recognizer should fail");
        assert!(
            failure.contains(
                r#"no recognizer "nope" is registered (registered: ["local", "openai", "echo"])"#
            ),
            "{failure}"
        );
    }

    #[test]
    fn audio_wav_writes_a_file_that_lasts_as_long_as_the_call() {
        let dir = PluginDir::new(&[(
            "file.lua",
            "dictate.recognizers.transform(function(list)
               table.insert(list, { id = 'file', transcribe = function(a)
                 local path = a:slice(2):wav()
                 local f = assert(io.open(path, 'rb'))
                 local bytes = f:read('a')
                 f:close()
                 return path .. ' ' .. #bytes
               end })
             end)",
        )]);
        let (raw, _) = with_recognizer(&dir, "file")
            .transcribe(vec![0.5, -0.5, 0.25, 1.0], |_| {
                Err("decode is not used".to_owned())
            })
            .expect("the file recognizer should transcribe");
        let (path, size) = raw.rsplit_once(' ').expect("path and size");
        // A 44-byte header and three 16-bit samples.
        assert_eq!(size, "50");
        let path = PathBuf::from(path);
        assert!(path.extension().is_some_and(|ext| ext == "wav"), "{path:?}");
        assert!(
            !path.exists(),
            "the WAV should be deleted when the call ends"
        );
        assert!(
            path.parent().is_some_and(|dir| !dir.exists()),
            "its directory should go with it"
        );

        let dir = PluginDir::new(&[(
            "empty.lua",
            "dictate.recognizers.transform(function(list)
               table.insert(list, { id = 'empty', transcribe = function(a) return a:slice(9):wav() end })
             end)",
        )]);
        let failure = with_recognizer(&dir, "empty")
            .transcribe(vec![0.5], |_| Err("decode is not used".to_owned()))
            .expect_err("empty audio has no WAV");
        assert!(failure.contains("the audio is empty"), "{failure}");
    }

    #[test]
    fn broken_recognizers_and_audio_hooks_fail_the_transcription() {
        for (code, recognizer, expected) in [
            (
                "dictate.recognizers.transform(function(list)
                   table.insert(list, { id = 'bad', transcribe = function() return 42 end })
                 end)",
                "bad",
                "recognizer bad returned a number, not a string",
            ),
            (
                "dictate.recognizers.transform(function(list)
                   table.insert(list, { id = 'bad', transcribe = function() error('engine down') end })
                 end)",
                "bad",
                "recognizer bad failed: ",
            ),
            (
                "dictate.on('audio', function(e) e.audio = 'loud' end)",
                "local",
                "an audio hook left event.audio as a string, not audio",
            ),
            (
                "dictate.on('audio', function(e) e.audio[5] = 1 end)",
                "local",
                "plugin x: audio hook failed: ",
            ),
        ] {
            let dir = PluginDir::new(&[("x.lua", code)]);
            let failure = with_recognizer(&dir, recognizer)
                .transcribe(vec![0.0; 4], |_| Ok("unreached".to_owned()))
                .expect_err("the transcription should fail");
            assert!(failure.contains(expected), "{expected}: {failure}");
        }

        let dir = PluginDir::new(&[(
            "x.lua",
            "dictate.recognizers.transform(function(list) table.insert(list, { id = 1 }) end)",
        )]);
        let host = with_recognizer(&dir, "local");
        assert!(!status(&host, "x").active);
        assert!(
            errors(&host, "x")
                .iter()
                .any(|error| error.contains("needs a string `id` and a `transcribe` function")),
            "{:?}",
            errors(&host, "x")
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
                 dictate.on('transcript', function(e) e.text = e.text .. ' (bad ran)' end)
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
            "dictate.on('transcript', function(e) e.text = 'hijacked' end)
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
            "dictate.on('transcript', function()
               local ok = pcall(dictate.on, 'transcript', function(e) e.text = 'late' end)
               assert(not ok, 'registering late should fail')
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
                "dictate.on('transcript', function(e) e.text = e.text .. '?' end)",
            ),
            (
                "formatter.lua",
                "dictate.on('transcript', function(e) e.text = '<' .. e.text .. '>' end)",
            ),
        ]);
        let mut host = dir.host();

        assert_eq!(process(&mut host, "hello period"), "<hello period>?");
        assert_eq!(
            host.plugins().len(),
            8,
            "recognizer, openai, formatter, agent, dictation, meeting, history, aaa"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exec_returns_output_and_reports_programs_that_cannot_run() {
        let dir = PluginDir::new(&[(
            "shell.lua",
            "dictate.on('transcript', function(e)
               local echoed = assert(dictate.exec({ 'sh', '-c', 'cat; echo err >&2; exit 3' }, { stdin = e.text }))
               local missing, why = dictate.exec({ 'no-such-program-for-dictate' })
               e.text = table.concat({
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
            "dictate.on('transcript', function(e)
               local result = dictate.exec(
                 {{ 'sh', '-c', 'sleep 30 & echo $! > {}; wait' }},
                 {{ timeout_ms = 300 }}
               )
               e.text = tostring(result.timed_out) .. ' ' .. tostring(result.code)
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
            "dictate.on('transcript', function(e)
               if e.text:find('Slow') then dictate.exec({ 'sleep', '0.5' }) end
               e.text = e.text .. ' done'
             end)",
        )]);
        let mut host = dir.host();
        let started = Instant::now();

        let slow = start(&mut host, "slow");
        let fast = start(&mut host, "fast");
        assert_eq!(
            finished(&mut host),
            vec![(fast, "Fast done".to_owned())],
            "only the fast chain should be done"
        );
        assert!(started.elapsed() < Duration::from_millis(400));

        let done = host
            .inbox
            .recv()
            .expect("the slow chain should be woken by its command");
        host.finish_exec(done.task, done.outcome);
        assert_eq!(finished(&mut host), vec![(slow, "Slow done".to_owned())]);
    }

    #[test]
    fn plugins_cannot_yield_the_hook_or_call_from_their_own_coroutines() {
        let dir = PluginDir::new(&[(
            "nested.lua",
            "dictate.on('transcript', function(e)
               local ok, why = pcall(coroutine.wrap(function() return dictate.exec({ 'true' }) end))
               e.text = e.text .. ' ' .. tostring(ok) .. ' ' .. (tostring(why):match('coroutine the plugin created') or tostring(why))
             end)",
        )]);
        let mut host = dir.host();
        assert_eq!(
            process(&mut host, "hi"),
            "Hi false coroutine the plugin created"
        );

        let dir = PluginDir::new(&[(
            "yield.lua",
            "dictate.on('transcript', function() coroutine.yield('x') end)",
        )]);
        let failure = dir
            .host()
            .process(&RawTranscript::new("hi"))
            .expect_err("yielding the hook should fail it");
        assert!(failure.contains("must not do"), "{failure}");
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
        let focus = api::Focus::from_snapshot(&stop_focus());
        host.process_for(&RawTranscript::new(raw), Some(&focus))
            .expect("the transcript hooks should succeed")
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
    /// The bundled `openai` recognizer, configured with `options`, on
    /// four samples.
    #[cfg(unix)]
    fn run_openai(dir: &PluginDir, options: serde_json::Value) -> Result<String, String> {
        let mut setup = setup(Some(dir.0.clone()));
        setup.recognizer = "openai".to_owned();
        setup.options.insert("openai".to_owned(), options);
        load(setup)
            .transcribe(vec![0.5, -0.5, 0.25, 1.0], |_| Err("not local".to_owned()))
            .map(|(raw, _)| raw)
    }

    #[cfg(unix)]
    #[test]
    fn the_openai_recognizer_posts_the_wav_with_the_key_on_stdin() {
        let dir = PluginDir::new(&[(
            "words.lua",
            "dictate.dictionary.transform(function(entries)
               for _, written in ipairs({ 'Rust', 'a<b', 'GPUI', '@/etc/passwd' }) do
                 table.insert(entries, { spoken = written:lower(), written = written })
               end
             end)",
        )]);
        // Stand-in for curl: check the request, then answer like the API.
        let curl = fake_agent(
            &dir,
            "curl",
            r#"headers=$(cat)
case "$headers" in "Authorization: Bearer $HOME") ;; *) echo "bad headers: $headers" >&2; exit 2 ;; esac
case "$*" in *"--header @-"*"--form-string model="*"http://stt.test/v1/audio/transcriptions") ;; *) echo "bad args: $*" >&2; exit 3 ;; esac
for arg; do
  case "$arg" in "$HOME"|*Bearer*|*Authorization*) echo "key in args" >&2; exit 4 ;; esac
done
kw=""; prev=""
for arg; do
  case "$arg" in file=@*) wav=${arg#file=@}; wav=${wav%;type=audio/wav} ;; esac
  case "$prev:$arg" in
    "--form-string:keywords[]="*) kw="$kw|${arg#keywords[]=}" ;;
    "--form-string:model="*) model=${arg#model=} ;;
  esac
  prev=$arg
done
printf '{"text":"  heard %s bytes by %s%s","usage":{"seconds":1}}' "$(wc -c < "$wav")" "$model" "$kw""#,
        );
        let options = |key_env: &str| {
            serde_json::json!({
                "program": curl,
                "url": "http://stt.test/v1/",
                "key_env": key_env,
            })
        };

        // A 44-byte header and four 16-bit samples, trimmed, then the
        // dictionary's written forms after transforms: once each, without
        // the one the API rejects, and `@` sent as text, not a file.
        let words = "|GPUI|Rust|@/etc/passwd";
        let run = |model: Option<&str>, keywords: Option<bool>| {
            let mut options = options("HOME");
            if let Some(model) = model {
                options["model"] = model.into();
            }
            if let Some(keywords) = keywords {
                options["keywords"] = keywords.into();
            }
            run_openai(&dir, options)
        };
        let heard = |model: &str, words: &str| Ok(format!("heard 52 bytes by {model}{words}"));
        assert_eq!(run(None, None), heard("gpt-transcribe", words));
        assert_eq!(run(None, Some(false)), heard("gpt-transcribe", ""));
        assert_eq!(run(Some("whisper-1"), None), heard("whisper-1", ""));
        assert_eq!(
            run(Some("whisper-1"), Some(true)),
            heard("whisper-1", words)
        );

        let failure = run_openai(&dir, options("DICTATE_TEST_NO_SUCH_KEY")).expect_err("no key");
        assert!(
            failure.contains("set DICTATE_TEST_NO_SUCH_KEY to your API key"),
            "{failure}"
        );

        let failing = fake_agent(&dir, "failing", "echo '{\"error\":\"bad key\"}'; exit 22");
        let mut options = options("HOME");
        options["program"] = failing.into();
        let failure = run_openai(&dir, options.clone()).expect_err("a failed request fails");
        assert!(failure.contains("curl exit 22"), "{failure}");
        assert!(failure.contains("bad key"), "{failure}");

        let plain = fake_agent(&dir, "plain", "cat >/dev/null; echo 'just text'");
        let mut plain_options = options;
        plain_options["program"] = plain.into();
        let failure = run_openai(&dir, plain_options).expect_err("a reply without JSON text fails");
        assert!(
            failure.contains("the transcription reply has no text: just text"),
            "{failure}"
        );
    }

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

    /// What the fake core records when it hears `text`: nothing for "".
    fn heard_audio(text: &str) -> core::Audio {
        core::Audio(vec![0.25; text.len() * 100])
    }

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
        dictate_in(&mut load(setup(None)), events, after_open, ended, heard)
    }

    /// [`dictate_once`] with plugins already loaded.
    fn dictate_in(
        host: &mut Plugins,
        events: &[PluginEvent],
        after_open: &[PluginEvent],
        ended: bool,
        heard: &str,
    ) -> (Vec<Delivered>, Vec<CoreCall>) {
        let mut delivered = Vec::new();
        let mut opened = false;
        let calls = drive(host, events, |call| match call {
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
                let recording = CoreReply::Recording {
                    audio: heard_audio(heard),
                    focus: stop_focus(),
                };
                (Ok(recording), Vec::new())
            }
            CoreCall::Decode { audio } => {
                assert_eq!(audio, &heard_audio(heard));
                let text = CoreReply::Text {
                    text: heard.to_owned(),
                };
                (Ok(text), Vec::new())
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
            CoreCall::DesktopFocus
            | CoreCall::CaptureOpen { .. }
            | CoreCall::CaptureRead { .. }
            | CoreCall::CaptureClose { .. } => (Err("unexpected".to_owned()), Vec::new()),
        });
        (delivered, calls)
    }

    #[test]
    fn bundled_history_shows_what_was_heard_and_typed_newest_first() {
        let data = PluginDir::new(&[]);
        let mut setup = setup(None);
        setup.data_dir = Some(data.0.clone());
        let mut host = load(setup);
        let press = [hotkey(HotkeyPhase::Down)];
        let release = [hotkey(HotkeyPhase::Up)];
        for heard in ["gee pee you eye works period", "ship it period"] {
            dictate_in(&mut host, &press, &release, false, heard);
        }

        let feed = host
            .render_page("history", &PageQuery::default())
            .expect("the history page should render");
        assert_eq!(feed.groups.len(), 1);
        assert_eq!(feed.groups[0].title, "Today");
        let rows: Vec<_> = feed.groups[0]
            .rows
            .iter()
            .map(|row| (row.text.as_str(), row.detail.as_deref()))
            .collect();
        assert_eq!(
            rows,
            [
                ("Ship it.", Some("Heard \u{201c}ship it period\u{201d}")),
                (
                    "GPUI works.",
                    Some("Heard \u{201c}gee pee you eye works period\u{201d}")
                ),
            ]
        );
        assert!(
            feed.groups[0].rows[0]
                .meta
                .as_deref()
                .is_some_and(|meta| meta.ends_with(" · notes.md")),
            "the row should name the window it went to"
        );

        let search = PageQuery {
            search: Some("gpui".to_owned()),
            limit: None,
        };
        let found = host
            .render_page("history", &search)
            .expect("search renders");
        assert_eq!(found.groups[0].rows.len(), 1);
        assert_eq!(found.groups[0].rows[0].text, "GPUI works.");

        let latest = PageQuery {
            search: None,
            limit: Some(1),
        };
        let latest = host.render_page("history", &latest).expect("limit renders");
        assert_eq!(latest.groups[0].rows.len(), 1);
        assert_eq!(latest.groups[0].rows[0].text, "Ship it.");

        let pages = host.pages();
        assert_eq!(
            pages,
            [PluginPage {
                id: "history".to_owned(),
                title: "History".to_owned(),
                plugin: "history".to_owned(),
                search: true,
            }]
        );
    }

    #[test]
    fn a_page_id_is_taken_once_and_a_failing_render_reports_its_error() {
        let dir = PluginDir::new(&[
            (
                "broken.lua",
                r#"dictate.ui.page("broken", "Broken", function() error("no data") end)"#,
            ),
            (
                "mine.lua",
                r#"dictate.ui.page("history", "Mine", function() return {} end)"#,
            ),
        ]);
        let mut host = dir.host();
        assert!(
            host.pages()
                .iter()
                .any(|page| page.id == "broken" && !page.search),
            "a page registered without options has no search box"
        );
        let error = host
            .render_page("broken", &PageQuery::default())
            .expect_err("a render that errors should fail");
        assert!(error.contains("no data"), "{error}");
        assert!(
            errors(&host, "mine")
                .iter()
                .any(|error| error.contains("already added a page \"history\"")),
            "a duplicate id should fail the later plugin: {:?}",
            errors(&host, "mine")
        );
        assert_eq!(
            host.pages()
                .iter()
                .map(|page| (page.id.as_str(), page.plugin.as_str()))
                .collect::<Vec<_>>(),
            [("history", "history"), ("broken", "broken")],
            "the bundled history page keeps its id"
        );
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
        assert_eq!(calls.len(), 4, "open, stop, decode, deliver: {calls:?}");
    }

    #[test]
    fn deliver_hooks_change_the_text_target_and_can_skip_delivery() {
        use dictate_desktop::DeliveryTarget::Clipboard;
        let press = [hotkey(HotkeyPhase::Down)];
        let release = [hotkey(HotkeyPhase::Up)];

        let dir = PluginDir::new(&[
            (
                "a-sign.lua",
                "dictate.on('deliver', function(e) e.text = e.text .. ' -- sent' end)",
            ),
            (
                "b-clip.lua",
                "dictate.on('deliver', function(e)
                   if e.focus and e.focus.app.id == 'dev.zed.Zed' then e.target = 'clipboard' end
                 end)",
            ),
        ]);
        let (delivered, _) = dictate_in(&mut dir.host(), &press, &release, false, "hi period");
        assert_eq!(
            delivered,
            vec![(
                "Hi. -- sent".to_owned(),
                Some(Clipboard),
                Some(stop_focus())
            )]
        );

        let dir = PluginDir::new(&[(
            "skip.lua",
            "dictate.on('deliver', function(e) e.text = '' end)",
        )]);
        let (delivered, calls) = dictate_in(&mut dir.host(), &press, &release, false, "hi period");
        assert!(delivered.is_empty(), "{delivered:?}");
        assert_eq!(calls.last(), Some(&CoreCall::OverlayHide));
    }

    #[test]
    fn a_failing_hook_loses_the_dictation_and_reports_it() {
        let dir = PluginDir::new(&[(
            "fail.lua",
            "dictate.on('deliver', function() error('deliver boom') end)",
        )]);
        let mut host = dir.host();
        let mut calls = drive(
            &mut host,
            &[hotkey(HotkeyPhase::Down), hotkey(HotkeyPhase::Up)],
            |call| match call {
                CoreCall::SessionOpen => (
                    Ok(CoreReply::Session {
                        session: SessionId(9),
                    }),
                    Vec::new(),
                ),
                CoreCall::SessionStop { .. } => (
                    Ok(CoreReply::Recording {
                        audio: heard_audio("hi period"),
                        focus: stop_focus(),
                    }),
                    Vec::new(),
                ),
                CoreCall::Decode { .. } => (
                    Ok(CoreReply::Text {
                        text: "hi period".to_owned(),
                    }),
                    Vec::new(),
                ),
                other @ (CoreCall::SessionCancel { .. }
                | CoreCall::DesktopFocus
                | CoreCall::DesktopDeliver { .. }
                | CoreCall::CaptureOpen { .. }
                | CoreCall::CaptureRead { .. }
                | CoreCall::CaptureClose { .. }
                | CoreCall::OverlayHide) => (Err(format!("unexpected {other:?}")), Vec::new()),
            },
        );
        calls.retain(|call| matches!(call, CoreCall::DesktopDeliver { .. }));
        assert!(calls.is_empty(), "{calls:?}");
        let failures = host.take_failures();
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].contains("deliver boom"), "{failures:?}");
        assert!(host.take_failures().is_empty());
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
            "dictate.on('transcript', function(e) e.text = e.text .. ' v1' end)",
        )]);
        let mut host = dir.host();
        assert_eq!(process(&mut host, "x"), "X v1");
        assert!(!host.reload_if_changed());

        dir.write(
            "tag.lua",
            "dictate.on('transcript', function(e) e.text = e.text .. ' version2' end)",
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
             dictate.on('transcript', function(e) n = n + 1; e.text = e.text .. ' a' .. n end)";
        let dir = PluginDir::new(&[
            ("a.lua", counter),
            (
                "b.lua",
                "dictate.on('transcript', function(e) e.text = e.text .. ' v1' end)",
            ),
        ]);
        let mut host = dir.host();
        assert_eq!(process(&mut host, "x"), "X a1 v1");

        // Only b reloads: a keeps its state.
        dir.write(
            "b.lua",
            "dictate.on('transcript', function(e) e.text = e.text .. ' version2' end)",
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
                "dictate.on('transcript', function(e) e.text = e.text .. ' tries=' .. tostring(TRIES) end)",
            ),
            (
                "b.lua",
                "dictate.on('transcript', function(e) e.text = e.text .. ' v1' end)",
            ),
        ]);
        let mut host = dir.host();
        assert_eq!(process(&mut host, "x"), "X tries=nil v1");

        dir.write(
            "b.lua",
            "TRIES = (TRIES or 0) + 1
             dictate.on('transcript', function(e) e.text = e.text .. ' broken' end)
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
            "dictate.on('transcript', function(e) e.text = e.text .. ' tries:' .. tostring(TRIES) end)",
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
                "dictate.on('transcript', function(e) dictate.exec({ 'sleep', '0.2' }) e.text = e.text .. ' a' end)",
            ),
            (
                "b.lua",
                "dictate.on('transcript', function(e) e.text = e.text .. ' b' end)",
            ),
        ]);
        let mut host = dir.host();
        let id = start(&mut host, "x");

        fs::remove_file(dir.0.join("b.lua")).expect("plugin file should be removed");
        assert!(host.reload_if_changed());
        let done = host.inbox.recv().expect("a's command should finish");
        host.finish_exec(done.task, done.outcome);

        assert_eq!(finished(&mut host), vec![(id, "X a".to_owned())]);
    }

    #[test]
    fn plugins_disabled_in_config_do_not_load() {
        let dir = PluginDir::new(&[(
            "shout.lua",
            "dictate.on('transcript', function(e) e.text = e.text:upper() end)",
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
            "dictate.on('transcript', function(e)
               local ok, why = pcall(dictate.desktop.focus)
               e.text = e.text .. ' ' .. tostring(ok) .. ' ' .. tostring(why):match('running daemon')
             end)",
        )]);
        let mut host = dir.host();

        assert_eq!(process(&mut host, "hi"), "Hi false running daemon");
    }

    /// The bundled `meeting.lua` against a fake core whose captures return
    /// `heard`: per source, each read's text, `""` for 2 s of near silence, or `None`
    /// once the capture ends. The mic's second read is held until a second
    /// `meeting` press stops the meeting. Returns the meeting file and
    /// every call made.
    fn meet(
        system_opens: bool,
        mic: Vec<Option<&'static str>>,
        system: Vec<Option<&'static str>>,
    ) -> (Option<String>, Vec<CoreCall>) {
        use crate::core::CaptureId;
        use crate::core::CaptureSource;
        const MIC: CaptureId = CaptureId(1);
        const SYSTEM: CaptureId = CaptureId(2);
        let press = PluginEvent::Hotkey {
            name: "meeting".to_owned(),
            phase: HotkeyPhase::Down,
            focus: None,
        };
        let data = PluginDir::new(&[]);
        let mut setup = setup(None);
        setup.data_dir = Some(data.0.clone());
        setup.options.insert(
            "meeting".to_owned(),
            serde_json::json!({ "segment_seconds": 2 }),
        );
        let mut host = load(setup);

        // Each read's audio is 2 s, its text found again by its first sample.
        let mut said: Vec<&'static str> = Vec::new();
        let mut reads = HashMap::from([(MIC, mic.into_iter()), (SYSTEM, system.into_iter())]);
        let mut mic_reads = 0;
        let mut held = None;
        let mut made = Vec::new();
        host.dispatch(&press);
        loop {
            let calls = host.take_calls();
            if calls.is_empty() {
                let Some((call, result)) = held.take() else {
                    break;
                };
                host.dispatch(&press);
                host.reply(call, result);
                continue;
            }
            for (call, op) in calls {
                let result = match &op {
                    CoreCall::CaptureOpen {
                        source: CaptureSource::Mic,
                    } => Ok(CoreReply::Capture { capture: MIC }),
                    CoreCall::CaptureOpen {
                        source: CaptureSource::System,
                    } if system_opens => Ok(CoreReply::Capture { capture: SYSTEM }),
                    CoreCall::CaptureOpen { .. } => Err("no loopback device".to_owned()),
                    CoreCall::CaptureRead { capture, .. } => {
                        match reads.get_mut(capture).and_then(Iterator::next).flatten() {
                            Some("") => Ok(CoreReply::Chunk {
                                audio: core::Audio(vec![0.001; 32_000]),
                            }),
                            Some(text) => {
                                said.push(text);
                                #[allow(clippy::cast_precision_loss)]
                                let marker = said.len() as f32;
                                Ok(CoreReply::Chunk {
                                    audio: core::Audio(vec![marker; 32_000]),
                                })
                            }
                            None => Err("unplugged".to_owned()),
                        }
                    }
                    CoreCall::Decode { audio } => {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let index = audio.0[0] as usize - 1;
                        Ok(CoreReply::Text {
                            text: said[index].to_owned(),
                        })
                    }
                    CoreCall::CaptureClose { .. } => Ok(CoreReply::Done),
                    other @ (CoreCall::SessionOpen
                    | CoreCall::SessionStop { .. }
                    | CoreCall::SessionCancel { .. }
                    | CoreCall::DesktopFocus
                    | CoreCall::DesktopDeliver { .. }
                    | CoreCall::OverlayHide) => Err(format!("unexpected {other:?}")),
                };
                if matches!(op, CoreCall::CaptureRead { capture: MIC, .. }) {
                    mic_reads += 1;
                }
                let stop_here =
                    mic_reads == 2 && matches!(op, CoreCall::CaptureRead { capture: MIC, .. });
                made.push(op);
                if stop_here {
                    held = Some((call, result));
                } else {
                    host.reply(call, result);
                }
            }
        }
        assert!(host.take_failures().is_empty());
        let file = fs::read_dir(data.0.join("meeting"))
            .ok()
            .and_then(|mut files| {
                let path = files.next()?.ok()?.path();
                fs::read_to_string(path).ok()
            });
        (file, made)
    }

    /// The meeting file without its dated heading.
    fn transcript(file: Option<String>) -> String {
        let file = file.expect("a meeting file");
        let (heading, rest) = file.split_once("\n\n").expect("a heading");
        assert!(heading.starts_with("# Meeting "), "{heading}");
        rest.to_owned()
    }

    #[test]
    fn bundled_meeting_labels_segments_by_source_until_stopped() {
        use crate::core::CaptureId;
        let (file, calls) = meet(
            true,
            vec![Some("Hello from me."), Some("Still me."), Some("")],
            vec![Some("Hello from them."), Some(""), Some("Bye.")],
        );
        assert_eq!(
            transcript(file),
            "**You** [00:00:00]: Hello from me.\n\n\
             **Call** [00:00:00]: Hello from them.\n\n\
             **You** [00:00:02]: Still me.\n\n\
             **Call** [00:00:04]: Bye.\n\n"
        );
        let reads = calls
            .iter()
            .filter_map(|call| {
                if let CoreCall::CaptureRead { capture, samples } = call {
                    Some((capture.0, *samples))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let decoded = calls
            .iter()
            .filter(|call| matches!(call, CoreCall::Decode { .. }))
            .count();
        assert_eq!(decoded, 4, "silence is not transcribed");
        // Two segments of 2 s, then what is left without waiting.
        assert_eq!(
            reads,
            [
                (1, 32_000),
                (2, 32_000),
                (1, 32_000),
                (2, 32_000),
                (1, 0),
                (2, 0)
            ]
        );
        assert_eq!(
            &calls[calls.len() - 2..],
            [
                CoreCall::CaptureClose {
                    capture: CaptureId(1)
                },
                CoreCall::CaptureClose {
                    capture: CaptureId(2)
                },
            ]
        );
    }

    #[test]
    fn bundled_meeting_keeps_going_without_system_audio_and_ends_with_the_mic() {
        use crate::core::CaptureId;
        let (file, calls) = meet(false, vec![Some("Just me."), None], Vec::new());
        assert_eq!(transcript(file), "**You** [00:00:00]: Just me.\n\n");
        assert_eq!(
            calls.last(),
            Some(&CoreCall::CaptureClose {
                capture: CaptureId(1)
            }),
            "an ended capture is still closed"
        );
    }
}
