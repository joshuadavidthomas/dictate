//! The `dictate` Lua API, declared once.
//!
//! Each [`ApiFunction`] both installs a function into the Lua VM and
//! describes its signature. Tables that cross the boundary (options,
//! results, hook contexts) are serde types that the runtime converts
//! through, and their `JsonSchema` (with doc comments) describes them. The
//! lua-language-server definitions in [`lua_types`] are generated from the
//! same declarations, so the editor types cannot drift from the runtime.

use std::collections::BTreeMap;

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
use crate::REGISTRIES;
use crate::active_plugin;
use crate::exec;
use crate::hook_budget;
use crate::json_to_lua;
use crate::register;
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
#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct App {
    /// Application identifier, such as `org.mozilla.firefox`.
    pub(crate) id: Option<String>,
    /// Window title.
    pub(crate) title: Option<String>,
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
    /// Dotted path under `dictate`, such as `json.encode`.
    path: String,
    doc: &'static str,
    params: Vec<Param>,
    returns: Vec<Return>,
    create: Create,
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
        create: Box::new(create),
    }
}

fn api() -> Vec<ApiFunction> {
    let mut functions = vec![
        function(
            "on",
            "Add a step to the per-utterance transcript chain. Return the new text, or nil to keep it. Only while the plugin file loads.",
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
        ),
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
            |lua| lua.create_function(exec::exec),
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

/// Install the `dictate` global.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let dictate = lua.create_table()?;
    for function in api() {
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

fn on(lua: &Lua, (event, hook, options): (String, Function, Option<Value>)) -> mlua::Result<()> {
    if event != "transcript" {
        return Err(mlua::Error::runtime(format!(
            "unknown event {event:?}; known events: \"transcript\""
        )));
    }
    let options: HookOptions = match options {
        Some(options) => lua.from_value(options)?,
        None => HookOptions::default(),
    };
    register(lua, HOOKS, hook, hook_budget(options.timeout_ms)?)
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

    lines.push("---@class dictate".to_owned());
    lines.push("dictate = {}".to_owned());
    lines.push(String::new());
    let mut tables = Vec::new();
    for function in &functions {
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
    let names = function
        .params
        .iter()
        .map(|param| param.name)
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!("function dictate.{}({names}) end", function.path));
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
