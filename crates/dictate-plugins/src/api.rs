//! The `dictate` Lua API, declared once.
//!
//! Each [`ApiFunction`] both installs a function into the Lua VM and
//! describes its signature. Tables that cross the boundary (options,
//! results, hook contexts) are serde types that the runtime converts
//! through, and their `JsonSchema` (with doc comments) describes them. The
//! lua-language-server definitions in [`lua_types`] are generated from the
//! same declarations, so the editor types cannot drift from the runtime.

use std::collections::BTreeMap;

use dictate_desktop::DeliveryTarget;
use dictate_desktop::FocusSnapshot;
use dictate_speech::DictationFormatter;
use dictate_speech::DictationMode;
use dictate_speech::RawTranscript;
use mlua::Function;
use mlua::Lua;
use mlua::LuaSerdeExt;
use mlua::Table;
use mlua::Value;
use schemars::JsonSchema;
use schemars::Schema;
use schemars::SchemaGenerator;
use serde::Deserialize;
use serde::Serialize;

use crate::CALL_BUDGET;
use crate::EffectiveContext;
use crate::Entry;
use crate::HOOKS;
use crate::HOTKEY_HANDLERS;
use crate::REGISTRIES;
use crate::TranscriptContext;
use crate::core::CoreCall;
use crate::core::CoreReply;
use crate::core::HotkeyPhase;
use crate::core::SessionId;
use crate::exec;
use crate::hook_budget;
use crate::json_to_lua;
use crate::register;
use crate::runtime::Call;
use crate::runtime::CoreReplyValue;
use crate::runtime::active_plugin;
use crate::runtime::core;
use crate::runtime::park;
use crate::transforms_key;

/// Options for `dictate.on`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HookOptions {
    /// Lua time this hook may take, 1 to 120000 ms (default 250). Time spent
    /// in `dictate.exec` does not count.
    pub(crate) timeout_ms: Option<u64>,
}

/// What a transcript hook knows about the dictation.
#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct HookContext {
    /// The recognizer's text, before any plugin ran.
    pub(crate) raw: String,
    pub(crate) mode: DictationMode,
    /// The window that had focus when recording stopped. Only known for
    /// insert delivery.
    pub(crate) app: Option<App>,
}

/// A focused window.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct App {
    /// Application identifier, such as `org.mozilla.firefox`.
    pub(crate) id: Option<String>,
    /// Window title.
    pub(crate) title: Option<String>,
}

/// A window that had focus.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Focus {
    /// The application, when the desktop reports it.
    pub(crate) app: Option<App>,
    /// Identifies the window for `dictate.desktop.deliver`'s focus check.
    /// Opaque.
    pub(crate) token: String,
}

impl Focus {
    pub(crate) fn from_snapshot(snapshot: &FocusSnapshot) -> Self {
        let app = match snapshot {
            FocusSnapshot::Focused(window) => Some(App {
                id: window.app_id().map(str::to_owned),
                title: window.title().map(str::to_owned),
            }),
            FocusSnapshot::NoFocusedWindow { .. } | FocusSnapshot::Unavailable => None,
        };
        Self {
            app,
            token: serde_json::to_string(snapshot).unwrap_or_default(),
        }
    }

    fn snapshot(&self) -> mlua::Result<FocusSnapshot> {
        serde_json::from_str(&self.token)
            .map_err(|error| mlua::Error::runtime(format!("not a focus from dictate: {error}")))
    }
}

/// A hotkey press or release.
#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct HotkeyEvent {
    /// The hotkey's name. `dictate` is the push-to-talk shortcut; `dictate
    /// hotkey <name> down|up` sends any name.
    pub(crate) name: String,
    pub(crate) phase: HotkeyPhase,
    /// The focused window when the key went up, when the core captured it.
    pub(crate) focus: Option<Focus>,
}

/// Options for `dictate.desktop.deliver`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeliverOptions {
    /// Where the text goes (default: the configured delivery).
    target: Option<DeliveryTarget>,
    /// For `insert`: the window the text is meant for. When focus has
    /// moved, or no window is given, the text goes to the clipboard.
    focus: Option<Focus>,
}

/// Options for `dictate.transcript.run`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunOptions {
    /// The window the text is for; hooks see its app as `ctx.app`.
    focus: Option<Focus>,
}

/// Options for `dictate.exec`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecOptions {
    /// 1 to 120000 ms (default 10000). The command and everything it
    /// started are killed when it runs out.
    pub(crate) timeout_ms: Option<u64>,
    /// Written to the command's stdin, which is then closed.
    pub(crate) stdin: Option<String>,
    /// Working directory.
    pub(crate) cwd: Option<String>,
}

/// What `dictate.exec` returns when the command ran.
#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct ExecResult {
    /// Exit code; nil when the command was killed.
    pub(crate) code: Option<i32>,
    /// Up to 8 MiB.
    #[schemars(with = "String")]
    #[serde(serialize_with = "lua_string")]
    pub(crate) stdout: Vec<u8>,
    /// Up to 8 MiB.
    #[schemars(with = "String")]
    #[serde(serialize_with = "lua_string")]
    pub(crate) stderr: Vec<u8>,
    pub(crate) timed_out: bool,
}

/// Command output becomes a Lua string as is, even when it is not UTF-8.
fn lua_string<S: serde::Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_bytes(bytes)
}

/// A Lua type in a generated signature.
enum LuaType {
    String,
    Any,
    /// A string literal, such as an event name.
    Literal(&'static str),
    Array(Box<LuaType>),
    Optional(Box<LuaType>),
    /// A class declared in [`classes`].
    Class(&'static str),
    /// A serde type, described by its JSON schema.
    Schema(fn(&mut SchemaGenerator) -> Schema),
    Function {
        params: Vec<(&'static str, LuaType)>,
        returns: Vec<LuaType>,
    },
}

fn schema<T: JsonSchema>() -> LuaType {
    LuaType::Schema(SchemaGenerator::subschema_for::<T>)
}

fn optional(inner: LuaType) -> LuaType {
    LuaType::Optional(Box::new(inner))
}

fn array(inner: LuaType) -> LuaType {
    LuaType::Array(Box::new(inner))
}

struct Param {
    name: &'static str,
    ty: LuaType,
    doc: &'static str,
}

fn param(name: &'static str, ty: LuaType, doc: &'static str) -> Param {
    Param { name, ty, doc }
}

struct Return {
    ty: LuaType,
    name: &'static str,
    doc: &'static str,
}

fn returns(ty: LuaType, name: &'static str, doc: &'static str) -> Return {
    Return { ty, name, doc }
}

struct ApiFunction {
    /// Dotted path under `dictate`, such as `json.encode`, or
    /// `Class:method` for a method of a class in [`classes`].
    path: String,
    doc: &'static str,
    params: Vec<Param>,
    returns: Vec<Return>,
    /// Other signatures, as `LuaType::Function`s.
    overloads: Vec<LuaType>,
    create: Create,
}

impl ApiFunction {
    fn overload(mut self, signature: LuaType) -> Self {
        self.overloads.push(signature);
        self
    }
}

/// A kind of object the API hands out, with methods declared as
/// `Name:method` functions.
struct ApiClass {
    name: &'static str,
    doc: &'static str,
}

fn classes() -> [ApiClass; 1] {
    [ApiClass {
        name: "Session",
        doc: "A recording from `dictate.session.open`.",
    }]
}

/// Creates one API function in a Lua VM.
type Create = Box<dyn Fn(&Lua) -> mlua::Result<Function>>;

fn function(
    path: impl Into<String>,
    doc: &'static str,
    params: Vec<Param>,
    returns: Vec<Return>,
    create: impl Fn(&Lua) -> mlua::Result<Function> + 'static,
) -> ApiFunction {
    ApiFunction {
        path: path.into(),
        doc,
        params,
        returns,
        overloads: Vec::new(),
        create: Box::new(create),
    }
}

// One declaration per API function; splitting the table would scatter it.
#[allow(clippy::too_many_lines)]
fn api() -> Vec<ApiFunction> {
    let mut functions = vec![
        function(
            "on",
            "Handle an event. A `transcript` hook is a step in the per-utterance chain: return the new text, or nil to keep it. A `hotkey` handler runs on each press and release, in its own coroutine. Only while the plugin file loads.",
            vec![
                param("event", LuaType::Literal("transcript"), ""),
                param(
                    "hook",
                    LuaType::Function {
                        params: vec![("text", LuaType::String), ("ctx", schema::<HookContext>())],
                        returns: vec![optional(LuaType::String)],
                    },
                    "",
                ),
                param("options", optional(schema::<HookOptions>()), ""),
            ],
            vec![],
            |lua| lua.create_function(on),
        )
        .overload(LuaType::Function {
            params: vec![
                ("event", LuaType::Literal("hotkey")),
                (
                    "handler",
                    LuaType::Function {
                        params: vec![("event", schema::<HotkeyEvent>())],
                        returns: vec![],
                    },
                ),
                ("options", optional(schema::<HookOptions>())),
            ],
            returns: vec![],
        }),
        function(
            "exec",
            "Run a command to completion with a timeout Dictate enforces.",
            vec![
                param(
                    "command",
                    array(LuaType::String),
                    "Program and arguments; no shell.",
                ),
                param("options", optional(schema::<ExecOptions>()), ""),
            ],
            vec![
                returns(optional(schema::<ExecResult>()), "result", ""),
                returns(
                    optional(LuaType::String),
                    "failure",
                    "Why the command could not start.",
                ),
            ],
            |lua| lua.create_async_function(exec::exec),
        ),
        function(
            "session.open",
            "Record from the microphone. While the `dictate` hotkey is held, this adopts the recording the core already started, and the transcript comes to this plugin instead of the built-in delivery. Otherwise it starts a recording. One recording runs at a time.",
            vec![],
            vec![
                returns(optional(LuaType::Class("Session")), "session", ""),
                returns(
                    optional(LuaType::String),
                    "failure",
                    "Why no recording could start.",
                ),
            ],
            |lua| lua.create_async_function(session_open),
        ),
        function(
            "Session:stop",
            "Stop recording, if the core has not already, and wait for the transcript.",
            vec![],
            vec![
                returns(LuaType::String, "text", "Empty when nothing was heard."),
                returns(
                    schema::<Focus>(),
                    "focus",
                    "The focused window when recording stopped.",
                ),
            ],
            |lua| lua.create_async_function(session_stop),
        ),
        function(
            "Session:cancel",
            "Stop recording and drop the audio.",
            vec![],
            vec![],
            |lua| lua.create_async_function(session_cancel),
        ),
        function(
            "desktop.focus",
            "The window that has focus now.",
            vec![],
            vec![returns(schema::<Focus>(), "", "")],
            |lua| lua.create_async_function(desktop_focus),
        ),
        function(
            "desktop.deliver",
            "Deliver text the way the core delivers dictation, including the focus check, clipboard transaction, and overlay.",
            vec![
                param("text", LuaType::String, ""),
                param("options", optional(schema::<DeliverOptions>()), ""),
            ],
            vec![
                returns(
                    optional(schema::<DeliveryTarget>()),
                    "target",
                    "Where the text went.",
                ),
                returns(optional(LuaType::String), "failure", ""),
            ],
            |lua| lua.create_async_function(desktop_deliver),
        ),
        function(
            "overlay.hide",
            "Hide the overlay, for example when a dictation ends without delivering anything.",
            vec![],
            vec![],
            |lua| lua.create_async_function(overlay_hide),
        ),
        function(
            "transcript.run",
            "Run the transcript hook chain (the formatter and every `transcript` hook) on `text`.",
            vec![
                param("text", LuaType::String, ""),
                param("options", optional(schema::<RunOptions>()), ""),
            ],
            vec![returns(LuaType::String, "", "")],
            |lua| lua.create_async_function(transcript_run),
        ),
        function(
            "log",
            "Write a line to the daemon log.",
            vec![param("message", LuaType::String, "")],
            vec![],
            |lua| lua.create_function(log),
        ),
        function(
            "text.format",
            "Run the built-in formatter with the current mode, dictionary, and replacements.",
            vec![param("text", LuaType::String, "")],
            vec![returns(LuaType::String, "", "")],
            |lua| lua.create_function(format),
        ),
        function(
            "json.encode",
            "Encode a Lua value as JSON.",
            vec![param("value", LuaType::Any, "")],
            vec![returns(LuaType::String, "", "")],
            |lua| lua.create_function(json_encode),
        ),
        function(
            "json.decode",
            "Decode JSON; `null` becomes nil.",
            vec![param("text", LuaType::String, "")],
            vec![returns(LuaType::Any, "", "")],
            |lua| lua.create_function(json_decode),
        ),
    ];
    for registry in REGISTRIES {
        functions.push(function(
            format!("{registry}.transform"),
            "Register a transform. Every rebuild starts from `config.toml` and runs each transform once, in plugin order, on a fresh copy; return a new list or edit it in place. Only while the plugin file loads.",
            vec![param(
                "transform",
                LuaType::Function {
                    params: vec![("entries", array(schema::<Entry>()))],
                    returns: vec![optional(array(schema::<Entry>()))],
                },
                "",
            )],
            vec![],
            move |lua| {
                let key = transforms_key(registry);
                lua.create_function(move |lua, transform: Function| {
                    register(lua, &key, transform, CALL_BUDGET)
                })
            },
        ));
    }
    functions
}

/// Install the `dictate` global and the classes' method tables.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let dictate = lua.create_table()?;
    for class in classes() {
        let metatable = lua.create_table()?;
        metatable.set("__index", lua.create_table()?)?;
        lua.set_named_registry_value(&class_key(class.name), metatable)?;
    }
    for function in api() {
        if let Some((class, method)) = function.path.split_once(':') {
            let methods: Table = lua
                .named_registry_value::<Table>(&class_key(class))?
                .get("__index")?;
            methods.set(method, (function.create)(lua)?)?;
            continue;
        }
        let mut table = dictate.clone();
        let mut segments = function.path.split('.').peekable();
        while let Some(segment) = segments.next() {
            if segments.peek().is_none() {
                table.set(segment, (function.create)(lua)?)?;
            } else {
                table = if let Some(next) = table.get::<Option<Table>>(segment)? {
                    next
                } else {
                    let next = lua.create_table()?;
                    table.set(segment, &next)?;
                    next
                };
            }
        }
    }
    lua.globals().set("dictate", dictate)
}

fn class_key(class: &str) -> String {
    format!("dictate.class.{class}")
}

/// A new object of `class` with `fields`.
fn object(lua: &Lua, class: &str, fields: Table) -> mlua::Result<Table> {
    fields.set_metatable(Some(lua.named_registry_value::<Table>(&class_key(class))?))?;
    Ok(fields)
}

fn on(lua: &Lua, (event, hook, options): (String, Function, Option<Value>)) -> mlua::Result<()> {
    let key = match event.as_str() {
        "transcript" => HOOKS,
        "hotkey" => HOTKEY_HANDLERS,
        _ => {
            return Err(mlua::Error::runtime(format!(
                "unknown event {event:?}; known events: \"transcript\", \"hotkey\""
            )));
        }
    };
    let options: HookOptions = match options {
        Some(options) => lua.from_value(options)?,
        None => HookOptions::default(),
    };
    register(lua, key, hook, hook_budget(options.timeout_ms)?)
}

fn unexpected(name: &str, reply: &CoreReply) -> mlua::Error {
    mlua::Error::runtime(format!(
        "{name}: the core sent an unexpected reply: {reply:?}"
    ))
}

fn focus_value(lua: &Lua, focus: &FocusSnapshot) -> mlua::Result<Value> {
    lua.to_value_with(&Focus::from_snapshot(focus), json_to_lua())
}

async fn session_open(lua: Lua, (): ()) -> mlua::Result<(Value, Value)> {
    const NAME: &str = "dictate.session.open";
    match core(&lua, NAME, CoreCall::SessionOpen).await? {
        Ok(CoreReply::Session { session }) => {
            let fields = lua.create_table()?;
            fields.set("id", session.0)?;
            Ok((Value::Table(object(&lua, "Session", fields)?), Value::Nil))
        }
        Ok(other) => Err(unexpected(NAME, &other)),
        Err(failure) => Ok((Value::Nil, Value::String(lua.create_string(failure)?))),
    }
}

fn session_id(session: &Table) -> mlua::Result<SessionId> {
    Ok(SessionId(session.get("id")?))
}

async fn session_stop(lua: Lua, session: Table) -> mlua::Result<(String, Value)> {
    const NAME: &str = "Session:stop";
    let session = session_id(&session)?;
    match core(&lua, NAME, CoreCall::SessionStop { session })
        .await?
        .map_err(mlua::Error::runtime)?
    {
        CoreReply::Transcript { text, focus } => Ok((text, focus_value(&lua, &focus)?)),
        other @ (CoreReply::Session { .. }
        | CoreReply::Focus { .. }
        | CoreReply::Delivered { .. }
        | CoreReply::Done) => Err(unexpected(NAME, &other)),
    }
}

async fn session_cancel(lua: Lua, session: Table) -> mlua::Result<()> {
    let session = session_id(&session)?;
    core(&lua, "Session:cancel", CoreCall::SessionCancel { session })
        .await?
        .map_err(mlua::Error::runtime)?;
    Ok(())
}

async fn desktop_focus(lua: Lua, (): ()) -> mlua::Result<Value> {
    const NAME: &str = "dictate.desktop.focus";
    match core(&lua, NAME, CoreCall::DesktopFocus)
        .await?
        .map_err(mlua::Error::runtime)?
    {
        CoreReply::Focus { focus } => focus_value(&lua, &focus),
        other @ (CoreReply::Session { .. }
        | CoreReply::Transcript { .. }
        | CoreReply::Delivered { .. }
        | CoreReply::Done) => Err(unexpected(NAME, &other)),
    }
}

async fn desktop_deliver(
    lua: Lua,
    (text, options): (String, Option<Value>),
) -> mlua::Result<(Value, Value)> {
    const NAME: &str = "dictate.desktop.deliver";
    let options: DeliverOptions = match options {
        Some(options) => lua.from_value(options)?,
        None => DeliverOptions::default(),
    };
    let focus = options.focus.as_ref().map(Focus::snapshot).transpose()?;
    let call = CoreCall::DesktopDeliver {
        text,
        target: options.target,
        focus,
    };
    match core(&lua, NAME, call).await? {
        Ok(CoreReply::Delivered { target }) => Ok((lua.to_value(&target)?, Value::Nil)),
        Ok(other) => Err(unexpected(NAME, &other)),
        Err(failure) => Ok((Value::Nil, Value::String(lua.create_string(failure)?))),
    }
}

async fn overlay_hide(lua: Lua, (): ()) -> mlua::Result<()> {
    core(&lua, "dictate.overlay.hide", CoreCall::OverlayHide)
        .await?
        .map_err(mlua::Error::runtime)?;
    Ok(())
}

async fn transcript_run(
    lua: Lua,
    (text, options): (String, Option<Value>),
) -> mlua::Result<String> {
    const NAME: &str = "dictate.transcript.run";
    let options: RunOptions = match options {
        Some(options) => lua.from_value(options)?,
        None => RunOptions::default(),
    };
    let app = options.focus.and_then(|focus| focus.app);
    let context = TranscriptContext {
        app_id: app.as_ref().and_then(|app| app.id.clone()),
        window_title: app.and_then(|app| app.title),
    };
    let values = park(&lua, NAME, Call::Chain { text, context }).await?;
    match values.front() {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        Some(Value::UserData(data)) => match data.take::<CoreReplyValue>()?.0 {
            Err(failure) => Err(mlua::Error::runtime(failure)),
            Ok(other) => Err(unexpected(NAME, &other)),
        },
        _ => Err(mlua::Error::runtime(format!(
            "{NAME} was resumed without text"
        ))),
    }
}

#[allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)] // mlua's callback shape
fn log(lua: &Lua, message: String) -> mlua::Result<()> {
    let plugin = active_plugin(lua).unwrap_or_else(|| "?".to_owned());
    eprintln!("plugin {plugin}: {message}");
    Ok(())
}

#[allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)] // mlua's callback shape
fn format(lua: &Lua, text: String) -> mlua::Result<String> {
    let context = lua
        .app_data_ref::<EffectiveContext>()
        .map(|context| context.0.borrow().clone())
        .unwrap_or_default();
    Ok(DictationFormatter
        .format(&RawTranscript::new(text), &context)
        .as_str()
        .to_owned())
}

fn json_encode(lua: &Lua, value: Value) -> mlua::Result<String> {
    let value: serde_json::Value = lua.from_value(value)?;
    serde_json::to_string(&value).map_err(mlua::Error::external)
}

#[allow(clippy::needless_pass_by_value)] // mlua's callback shape
fn json_decode(lua: &Lua, text: mlua::LuaString) -> mlua::Result<Value> {
    let value: serde_json::Value =
        serde_json::from_slice(&text.as_bytes()).map_err(mlua::Error::external)?;
    lua.to_value_with(&value, json_to_lua())
}

/// lua-language-server definitions for the `dictate` API, generated from
/// the same declarations that install it.
#[must_use]
pub fn lua_types() -> String {
    let functions = api();
    let mut generator = SchemaGenerator::default();
    // Render signatures first: that registers every schema type they use.
    let signatures = functions
        .iter()
        .map(|function| render_function(function, &mut generator))
        .collect::<Vec<_>>();

    let mut lines = vec![
        "---@meta dictate".to_owned(),
        "-- Generated from crates/dictate-plugins/src/api.rs; do not edit.".to_owned(),
        "-- `dictate plugins --types` installs this file next to your plugins.".to_owned(),
        String::new(),
    ];
    let definitions = generator.definitions().iter().collect::<BTreeMap<_, _>>();
    for (name, schema) in definitions {
        render_definition(&mut lines, name, schema);
    }

    for class in classes() {
        lines.extend(class.doc.lines().map(|line| format!("---{line}")));
        lines.push(format!("---@class dictate.{}", class.name));
        lines.push(format!("local {} = {{}}", class.name));
        lines.push(String::new());
    }

    lines.push("---@class dictate".to_owned());
    lines.push("dictate = {}".to_owned());
    lines.push(String::new());
    let mut tables = Vec::new();
    for function in functions
        .iter()
        .filter(|function| !function.path.contains(':'))
    {
        if let Some((table, _)) = function.path.rsplit_once('.')
            && !tables.contains(&table)
        {
            tables.push(table);
            lines.push(format!("dictate.{table} = {{}}"));
        }
    }
    lines.push(String::new());
    for signature in signatures {
        lines.extend(signature);
        lines.push(String::new());
    }
    lines
        .iter()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_function(function: &ApiFunction, generator: &mut SchemaGenerator) -> Vec<String> {
    let mut lines = function
        .doc
        .lines()
        .map(|line| format!("---{line}"))
        .collect::<Vec<_>>();
    for param in &function.params {
        let (ty, optional) = lua_type(&param.ty, generator);
        let mark = if optional { "?" } else { "" };
        lines.push(format!("---@param {}{mark} {ty} {}", param.name, param.doc));
    }
    for ret in &function.returns {
        let (ty, optional) = lua_type(&ret.ty, generator);
        let mark = if optional { "?" } else { "" };
        lines.push(format!("---@return {ty}{mark} {} {}", ret.name, ret.doc));
    }
    for overload in &function.overloads {
        lines.push(format!("---@overload {}", lua_type(overload, generator).0));
    }
    let names = function
        .params
        .iter()
        .map(|param| param.name)
        .collect::<Vec<_>>()
        .join(", ");
    let owner = if function.path.contains(':') {
        ""
    } else {
        "dictate."
    };
    lines.push(format!("function {owner}{}({names}) end", function.path));
    lines
}

/// The Lua type, and whether it may be nil.
fn lua_type(ty: &LuaType, generator: &mut SchemaGenerator) -> (String, bool) {
    match ty {
        LuaType::String => ("string".to_owned(), false),
        LuaType::Any => ("any".to_owned(), false),
        LuaType::Literal(value) => (format!("{value:?}"), false),
        LuaType::Array(inner) => (format!("{}[]", lua_type(inner, generator).0), false),
        LuaType::Optional(inner) => (lua_type(inner, generator).0, true),
        LuaType::Class(name) => (format!("dictate.{name}"), false),
        LuaType::Schema(subschema) => schema_type(subschema(generator).as_value()),
        LuaType::Function { params, returns } => {
            let params = params
                .iter()
                .map(|(name, ty)| {
                    let (ty, optional) = lua_type(ty, generator);
                    format!("{name}: {ty}{}", if optional { "?" } else { "" })
                })
                .collect::<Vec<_>>()
                .join(", ");
            let returns = returns
                .iter()
                .map(|ty| {
                    let (ty, optional) = lua_type(ty, generator);
                    format!("{ty}{}", if optional { "?" } else { "" })
                })
                .collect::<Vec<_>>()
                .join(", ");
            let returns = if returns.is_empty() {
                String::new()
            } else {
                format!(": {returns}")
            };
            (format!("fun({params}){returns}"), false)
        }
    }
}

/// The Lua type a JSON schema describes, and whether it may be nil.
///
/// Panics on a schema shape it does not know, so a new API type that the
/// generator cannot render fails the staleness test instead of producing
/// a wrong definition.
fn schema_type(schema: &serde_json::Value) -> (String, bool) {
    if let Some(reference) = schema.get("$ref").and_then(serde_json::Value::as_str) {
        let name = reference.rsplit('/').next().unwrap_or(reference);
        return (format!("dictate.{name}"), false);
    }
    if let Some(variants) = schema.get("anyOf").and_then(serde_json::Value::as_array) {
        let (nulls, types): (Vec<_>, Vec<_>) = variants.iter().partition(|variant| {
            variant.get("type").and_then(serde_json::Value::as_str) == Some("null")
        });
        let types = types.into_iter().map(|variant| schema_type(variant).0);
        return (types.collect::<Vec<_>>().join("|"), !nulls.is_empty());
    }
    if let Some(values) = schema.get("enum").and_then(serde_json::Value::as_array) {
        let literals = values.iter().map(ToString::to_string);
        return (literals.collect::<Vec<_>>().join("|"), false);
    }
    let types = match schema.get("type") {
        Some(serde_json::Value::String(ty)) => vec![ty.as_str()],
        Some(serde_json::Value::Array(types)) => {
            types.iter().filter_map(serde_json::Value::as_str).collect()
        }
        other => panic!("cannot render schema type {other:?} in {schema}"),
    };
    let rendered = types
        .iter()
        .filter(|ty| **ty != "null")
        .map(|ty| match *ty {
            "string" | "integer" | "number" | "boolean" => (*ty).to_owned(),
            "array" => format!("{}[]", schema_type(&schema["items"]).0),
            other => panic!("cannot render schema type {other:?} in {schema}"),
        })
        .collect::<Vec<_>>()
        .join("|");
    (rendered, types.contains(&"null"))
}

fn render_definition(lines: &mut Vec<String>, name: &str, schema: &serde_json::Value) {
    let description = schema
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    lines.extend(description.lines().map(|line| format!("---{line}")));
    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        let required = schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .map(|required| {
                required
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        lines.push(format!("---@class dictate.{name}"));
        for (field, property) in properties {
            let (ty, nullable) = schema_type(property);
            let optional = nullable || !required.contains(&field.as_str());
            let doc = property
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .replace('\n', " ");
            let mark = if optional { "?" } else { "" };
            lines.push(format!("---@field {field}{mark} {ty} {doc}"));
        }
    } else {
        let (ty, _) = schema_type(schema);
        lines.push(format!("---@alias dictate.{name} {ty}"));
    }
    lines.push(String::new());
}
