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
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use dictate_speech::CustomDictionary;
use dictate_speech::DictationContext;
use dictate_speech::DictationFormatter;
use dictate_speech::DictationMode;
use dictate_speech::ProcessedDictation;
use dictate_speech::RawTranscript;
use dictate_speech::ReplacementRule;
use dictate_speech::SpokenFormatting;
use mlua::FromLuaMulti;
use mlua::Function;
use mlua::HookTriggers;
use mlua::IntoLuaMulti;
use mlua::Lua;
use mlua::LuaSerdeExt;
use mlua::MultiValue;
use mlua::Table;
use mlua::Value;
use mlua::VmState;
use mlua::thread::ThreadStatus;
use serde::Deserialize;
use serde::Serialize;

mod exec;
mod process;

pub use process::HostCommand;
pub use process::PluginProcess;
pub use process::serve;

/// Time a plugin file gets to run its top-level code.
const LOAD_BUDGET: Duration = Duration::from_secs(1);
/// Time one transform call, or a hook without a `timeout_ms`, gets before it
/// is stopped.
const CALL_BUDGET: Duration = Duration::from_millis(250);
/// The longest `timeout_ms` a hook may ask for.
const MAX_HOOK_BUDGET: Duration = Duration::from_mins(2);
/// How often, in VM instructions, the budget is checked.
const BUDGET_CHECK_INSTRUCTIONS: u32 = 1_000;

/// lua-language-server definitions for the `dictate` API.
pub const LUA_TYPES: &str = include_str!("../meta/dictate.lua");

const BUNDLED: [(&str, &str); 2] = [
    ("formatter", include_str!("../plugins/formatter.lua")),
    ("agent", include_str!("../plugins/agent.lua")),
];

const HOOKS: &str = "dictate.hooks";
const DICTIONARY: &str = "dictionary";
const REPLACEMENTS: &str = "replacements";
const REGISTRIES: [&str; 2] = [DICTIONARY, REPLACEMENTS];

/// One spoken-to-written pair in the `dictionary` or `replacements`
/// registry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
pub(crate) struct PluginHost {
    lua: Lua,
    setup: PluginSetup,
    plugins: Vec<PluginStatus>,
}

/// One hook or transform, with the plugin that registered it.
struct Registration {
    plugin: String,
    function: Function,
    budget: Duration,
}

/// Deadline for the plugin call in progress, checked by the VM hook.
struct Deadline(Cell<Option<Instant>>);
/// The plugin whose code is running, if any.
struct ActivePlugin(RefCell<Option<String>>);
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
    fn load(setup: PluginSetup, reporter: Option<exec::ExecReporter>) -> mlua::Result<Self> {
        let lua = Lua::new();
        if let Some(reporter) = reporter {
            lua.set_app_data(reporter);
        }
        install_api(&lua, setup.base_context())?;

        let plugins = plugin_sources(&fingerprint(setup.plugins_dir.as_deref()))
            .into_iter()
            .map(|source| {
                let options = setup.options.get(&source.id);
                load_plugin(&lua, source, options)
            })
            .collect();

        let mut host = Self {
            lua,
            setup,
            plugins,
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

    /// Run the transcript hook chain. A hook that errors, overruns its
    /// budget, or returns something other than a string or nil is skipped
    /// and the chain continues with the text it was given.
    fn process(
        &mut self,
        raw: &RawTranscript,
        transcript: &TranscriptContext,
    ) -> ProcessedDictation {
        let mut text = raw.as_str().to_owned();
        let hooks = match registrations(&self.lua, HOOKS) {
            Ok(hooks) => hooks,
            Err(error) => {
                eprintln!("plugin hooks are unavailable, using the raw transcript: {error}");
                return ProcessedDictation::new(text);
            }
        };

        for Registration {
            plugin,
            function: hook,
            budget,
        } in hooks
        {
            let outcome = self.lua.create_table().and_then(|context| {
                context.set("raw", raw.as_str())?;
                context.set("mode", self.setup.mode.id())?;
                if transcript.app_id.is_some() || transcript.window_title.is_some() {
                    let app = self.lua.create_table()?;
                    app.set("id", transcript.app_id.as_deref())?;
                    app.set("title", transcript.window_title.as_deref())?;
                    context.set("app", app)?;
                }
                call_plugin::<Value>(&self.lua, &plugin, budget, &hook, (text.as_str(), context))
            });
            match outcome {
                Ok(Value::String(next)) => text = next.to_string_lossy(),
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

        ProcessedDictation::new(text.trim().to_owned())
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
    lua.set_app_data(Deadline(Cell::new(None)));
    lua.set_app_data(ActivePlugin(RefCell::new(None)));
    lua.set_app_data(Loading(Cell::new(false)));
    lua.set_app_data(EffectiveContext(RefCell::new(context)));

    lua.set_named_registry_value(HOOKS, lua.create_table()?)?;
    for name in REGISTRIES {
        lua.set_named_registry_value(&transforms_key(name), lua.create_table()?)?;
    }

    let dictate = lua.create_table()?;
    dictate.set(
        "on",
        lua.create_function(
            |lua, (event, hook, options): (String, Function, Option<Table>)| {
                if event != "transcript" {
                    return Err(mlua::Error::runtime(format!(
                        "unknown event {event:?}; known events: \"transcript\""
                    )));
                }
                let budget = match options {
                    Some(options) => hook_budget(options.get::<Option<u64>>("timeout_ms")?)?,
                    None => CALL_BUDGET,
                };
                register(lua, HOOKS, hook, budget)
            },
        )?,
    )?;
    for name in REGISTRIES {
        let registry = lua.create_table()?;
        let key = transforms_key(name);
        registry.set(
            "transform",
            lua.create_function(move |lua, transform: Function| {
                register(lua, &key, transform, CALL_BUDGET)
            })?,
        )?;
        dictate.set(name, registry)?;
    }

    let text = lua.create_table()?;
    text.set(
        "format",
        lua.create_function(|lua, text: String| {
            let context = lua
                .app_data_ref::<EffectiveContext>()
                .map(|context| context.0.borrow().clone())
                .unwrap_or_default();
            Ok(DictationFormatter
                .format(&RawTranscript::new(text), &context)
                .as_str()
                .to_owned())
        })?,
    )?;
    dictate.set("text", text)?;

    dictate.set("exec", lua.create_function(exec::exec)?)?;
    let json = lua.create_table()?;
    json.set(
        "encode",
        lua.create_function(|lua, value: Value| {
            let value: serde_json::Value = lua.from_value(value)?;
            serde_json::to_string(&value).map_err(mlua::Error::external)
        })?,
    )?;
    json.set(
        "decode",
        lua.create_function(|lua, text: mlua::LuaString| {
            let value: serde_json::Value =
                serde_json::from_slice(&text.as_bytes()).map_err(mlua::Error::external)?;
            lua.to_value_with(&value, json_to_lua())
        })?,
    )?;
    dictate.set("json", json)?;
    dictate.set(
        "log",
        lua.create_function(|lua, message: String| {
            let plugin = active_plugin(lua).unwrap_or_else(|| "?".to_owned());
            eprintln!("plugin {plugin}: {message}");
            Ok(())
        })?,
    )?;

    lua.globals().set("dictate", dictate)
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
    let Some(plugin) = active_plugin(lua).filter(|_| loading) else {
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
    let keys = std::iter::once(HOOKS.to_owned()).chain(REGISTRIES.into_iter().map(transforms_key));
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

/// Call plugin code with `plugin` marked active and a deadline armed.
///
/// The call runs in its own coroutine. At the deadline the instruction hook
/// suspends that coroutine, and it is never resumed. A suspension, unlike an
/// error, cannot be caught by the plugin's own `pcall`, so a runaway call
/// always ends. Time spent inside a single C function, such as one huge
/// `string.rep`, is not interrupted.
fn call_plugin<R: FromLuaMulti>(
    lua: &Lua,
    plugin: &str,
    budget: Duration,
    function: &Function,
    args: impl IntoLuaMulti,
) -> mlua::Result<R> {
    let thread = lua.create_thread(function.clone())?;
    thread.set_hook(
        HookTriggers::new().every_nth_instruction(BUDGET_CHECK_INSTRUCTIONS),
        |lua, _| {
            let expired = lua
                .app_data_ref::<Deadline>()
                .and_then(|deadline| deadline.0.get())
                .is_some_and(|deadline| Instant::now() >= deadline);
            Ok(if expired {
                VmState::Yield
            } else {
                VmState::Continue
            })
        },
    )?;

    if let Some(active) = lua.app_data_ref::<ActivePlugin>() {
        *active.0.borrow_mut() = Some(plugin.to_owned());
    }
    if let Some(deadline) = lua.app_data_ref::<Deadline>() {
        deadline.0.set(Some(Instant::now() + budget));
    }
    let result = thread.resume::<MultiValue>(args);
    if let Some(deadline) = lua.app_data_ref::<Deadline>() {
        deadline.0.set(None);
    }
    if let Some(active) = lua.app_data_ref::<ActivePlugin>() {
        *active.0.borrow_mut() = None;
    }

    let values = result?;
    if thread.status() == ThreadStatus::Resumable {
        return Err(mlua::Error::runtime(format!(
            "stopped after exceeding its {} ms time budget (or yielded, which plugin calls must not do)",
            budget.as_millis()
        )));
    }
    R::from_lua_multi(values, lua)
}

fn active_plugin(lua: &Lua) -> Option<String> {
    lua.app_data_ref::<ActivePlugin>()
        .and_then(|active| active.0.borrow().clone())
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
        assert_eq!(host.plugins().len(), 3, "formatter, agent, aaa");
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

    /// Every function under `dictate`, as dotted paths.
    fn api_functions(lua: &Lua) -> Vec<String> {
        let mut paths: Vec<String> = lua
            .load(
                "local paths = {}
                 local function walk(t, prefix)
                   for key, value in pairs(t) do
                     local path = prefix .. '.' .. key
                     if type(value) == 'function' then table.insert(paths, path)
                     elseif type(value) == 'table' then walk(value, path) end
                   end
                 end
                 walk(dictate, 'dictate')
                 return paths",
            )
            .eval()
            .expect("the dictate table should be walkable");
        paths.sort();
        paths
    }

    #[test]
    fn the_type_definitions_cover_exactly_the_runtime_api() {
        let host = PluginHost::load(setup(None), None).expect("plugin host should load");
        let types = Lua::new();
        types
            .load(LUA_TYPES)
            .exec()
            .expect("the type definitions should be valid Lua");

        assert_eq!(api_functions(&types), api_functions(&host.lua));
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
        fn process(&self) -> PluginProcess {
            PluginProcess::new(
                Box::new(|| {
                    let mut command = std::process::Command::new(std::env::current_exe()?);
                    command
                        .args(["--exact", "tests::plugin_host_child", "--nocapture"])
                        .env(CHILD, "1");
                    Ok(command)
                }),
                setup(Some(self.0.clone())),
            )
        }
    }

    fn run(process: &mut PluginProcess, raw: &str) -> String {
        process
            .process(&RawTranscript::new(raw), &TranscriptContext::default())
            .as_str()
            .to_owned()
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
        assert_eq!(process.plugins().len(), 3, "formatter, agent, noisy");
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
