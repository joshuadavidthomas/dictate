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
use dictate_speech::CapturedUtterance;
use dictate_speech::DICTATION_SAMPLE_RATE;
use dictate_speech::DictationCommand;
use dictate_speech::DictationFormatter;
use dictate_speech::DictationMode;
use dictate_speech::RawTranscript;
use dictate_speech::save_wav_utterance;
use mlua::AnyUserData;
use mlua::Function;
use mlua::Lua;
use mlua::LuaSerdeExt;
use mlua::MetaMethod;
use mlua::Table;
use mlua::UserData;
use mlua::UserDataFields;
use mlua::UserDataMethods;
use mlua::Value;
use schemars::JsonSchema;
use schemars::Schema;
use schemars::SchemaGenerator;
use serde::Deserialize;
use serde::Serialize;

use crate::AUDIO_HOOKS;
use crate::DELIVER_HOOKS;
use crate::DataDir;
use crate::EffectiveContext;
use crate::Entry;
use crate::HOTKEY_HANDLERS;
use crate::RECOGNIZERS;
use crate::RECORD_HANDLERS;
use crate::REGISTRIES;
use crate::RegistryEntries;
use crate::SESSION_HANDLERS;
use crate::SelectedRecognizer;
use crate::TRANSCRIPT_HOOKS;
use crate::core::Audio;
use crate::core::CaptureId;
use crate::core::CaptureSource;
use crate::core::CoreCall;
use crate::core::CoreReply;
use crate::core::HotkeyPhase;
use crate::core::SessionId;
use crate::exec;
use crate::json_to_lua;
use crate::register;
use crate::register_page;
use crate::runtime::active_plugin;
use crate::runtime::core;
use crate::runtime::hooks;
use crate::transforms_key;

/// `audio[i]` reads and writes sample `i`, from 1; `#audio` is the length.
impl UserData for Audio {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field("rate", DICTATION_SAMPLE_RATE.as_hz());
    }

    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Len, |_, audio, ()| Ok(audio.0.len()));
        methods.add_meta_method(MetaMethod::Index, |_, audio, index: i64| {
            Ok(sample_index(audio, index).map(|index| audio.0[index]))
        });
        methods.add_meta_method_mut(
            MetaMethod::NewIndex,
            |_, audio, (index, sample): (i64, f32)| {
                let Some(index) = sample_index(audio, index) else {
                    return Err(mlua::Error::runtime(format!(
                        "audio index {index} is outside 1..{}",
                        audio.0.len()
                    )));
                };
                audio.0[index] = sample;
                Ok(())
            },
        );
        methods.add_method("wav", |lua, audio, ()| {
            let path = crate::runtime::temp_file(lua, "Audio:wav", "wav")?;
            let utterance = CapturedUtterance::new(DICTATION_SAMPLE_RATE, audio.0.clone())
                .ok_or_else(|| mlua::Error::runtime("Audio:wav: the audio is empty"))?;
            save_wav_utterance(&path, &utterance)
                .map_err(|error| mlua::Error::runtime(format!("Audio:wav: {error:#}")))?;
            Ok(path.to_string_lossy().into_owned())
        });
        methods.add_method("slice", |_, audio, (from, to): (i64, Option<i64>)| {
            let len = i64::try_from(audio.0.len()).unwrap_or(i64::MAX);
            let from = usize::try_from(from.clamp(1, len + 1) - 1).unwrap_or(0);
            let to = usize::try_from(to.unwrap_or(len).clamp(0, len)).unwrap_or(0);
            Ok(Audio(audio.0.get(from..to).unwrap_or_default().to_vec()))
        });
    }
}

/// The zero-based position of Lua index `index`, when it is in range.
fn sample_index(audio: &Audio, index: i64) -> Option<usize> {
    usize::try_from(index)
        .ok()
        .and_then(|index| index.checked_sub(1))
        .filter(|index| *index < audio.0.len())
}

/// A transcript on its way through the `transcript` hooks.
#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct TranscriptEvent {
    /// The text so far. Assign to it to change the transcript; later hooks
    /// see the change. `""` means nothing is delivered.
    text: String,
    /// The text `dictate.transcript.run` was given, before any hook ran.
    raw: String,
    mode: DictationMode,
    /// The window the text is for, when known.
    focus: Option<Focus>,
}

/// Text on its way through the `deliver` hooks to `dictate.desktop.deliver`.
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DeliverEvent {
    /// The text to deliver. `""` skips delivery.
    text: String,
    /// Where it goes; nil means the configured delivery.
    target: Option<DeliveryTarget>,
    /// For `insert`: the window the text is meant for.
    focus: Option<Focus>,
}

/// Options for `dictate.ui.page`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageOptions {
    /// Show a search box above the page; what is typed arrives as `query.search`.
    #[serde(default)]
    pub(crate) search: bool,
}

/// What a page's `render` is asked to show.
#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PageQuery {
    /// Text to look for; nil shows everything.
    pub search: Option<String>,
    /// Show at most this many rows; nil leaves it to the page.
    pub limit: Option<u32>,
}

/// A page's content: rows under headings, in the order given.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Feed {
    #[serde(default)]
    pub groups: Vec<FeedGroup>,
    /// Shown instead of the groups when there are none.
    pub empty: Option<String>,
}

/// A heading and the rows under it, such as one day.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FeedGroup {
    pub title: String,
    #[serde(default)]
    pub rows: Vec<FeedRow>,
}

/// One entry in a feed.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FeedRow {
    pub text: String,
    /// A quieter second line, such as what was heard before formatting.
    pub detail: Option<String>,
    /// A short note beside the row, such as the time.
    pub meta: Option<String>,
}

/// A focused window.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct App {
    /// Application identifier, such as `org.mozilla.firefox`.
    id: Option<String>,
    /// Window title.
    title: Option<String>,
}

/// A window that had focus.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Focus {
    /// The application, when the desktop reports it.
    app: Option<App>,
    /// Identifies the window for `dictate.desktop.deliver`'s focus check.
    /// Opaque.
    token: String,
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

/// `dictate record <command>` from the command line or a compositor binding.
#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct RecordEvent {
    pub(crate) command: DictationCommand,
    /// `--delivery`, when given.
    pub(crate) target: Option<DeliveryTarget>,
    /// The focused window when the command was sent, when known.
    pub(crate) focus: Option<Focus>,
}

/// Something that happened to a session.
#[derive(Debug, JsonSchema, Serialize)]
pub(crate) struct SessionEvent {
    /// The session's `id`.
    pub(crate) id: u64,
    /// `ended`: the recording ended without `Session:stop`, for example at
    /// the length limit. `Session:stop` returns its transcript.
    pub(crate) kind: SessionEventKind,
}

#[derive(Debug, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SessionEventKind {
    Ended,
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
    /// The window the text is for; hooks see it as `event.focus`.
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
    Number,
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
    /// lua-language-server `---@field` declarations: name, type, doc.
    fields: &'static [(&'static str, &'static str, &'static str)],
}

fn classes() -> [ApiClass; 5] {
    [
        ApiClass {
            name: "Capture",
            doc: "Audio from `dictate.capture.open`, recorded apart from dictation until closed.",
            fields: &[
                ("id", "integer", ""),
                ("source", "dictate.CaptureSource", ""),
            ],
        },
        ApiClass {
            name: "Session",
            doc: "A recording from `dictate.session.open`.",
            fields: &[(
                "id",
                "integer",
                "Identifies the session in `session` events.",
            )],
        },
        ApiClass {
            name: "Audio",
            doc: "Recorded samples, mono, from -1 to 1. `audio[i]` reads or writes sample `i` (from 1) and `#audio` is the number of samples.",
            fields: &[
                ("rate", "integer", "Samples per second: 16000."),
                ("[integer]", "number", ""),
                (
                    "slice",
                    "fun(self: dictate.Audio, from: integer, to?: integer): dictate.Audio",
                    "A copy of samples `from` through `to` (default: the end).",
                ),
                (
                    "wav",
                    "fun(self: dictate.Audio): string",
                    "Write the audio to a 16 kHz, 16-bit mono WAV file and return its path, for engines that take a file. The file is deleted when the hook or handler that wrote it ends.",
                ),
            ],
        },
        ApiClass {
            name: "AudioEvent",
            doc: "A recording on its way through the `audio` hooks to the recognizer.",
            fields: &[(
                "audio",
                "dictate.Audio",
                "Edit its samples, or assign other audio, such as a slice.",
            )],
        },
        ApiClass {
            name: "Recognizer",
            doc: "A speech-to-text engine in the `recognizers` registry.",
            fields: &[
                ("id", "string", "What the `recognizer` setting names."),
                (
                    "transcribe",
                    "fun(audio: dictate.Audio): string",
                    "The text heard in `audio`; empty when nothing was said.",
                ),
            ],
        },
    ]
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
            "Handle an event. `audio`, `transcript`, and `deliver` hooks run in plugin order on one event table: change its fields, and later hooks see the change. A hook that errors fails the `dictate.transcribe`, `dictate.transcript.run`, or `dictate.desktop.deliver` call that ran it. `hotkey`, `record`, and `session` handlers run in their own coroutines. Only while the plugin file loads.",
            vec![
                param("event", LuaType::Literal("transcript"), ""),
                param(
                    "hook",
                    LuaType::Function {
                        params: vec![("event", schema::<TranscriptEvent>())],
                        returns: vec![],
                    },
                    "",
                ),
            ],
            vec![],
            |lua| lua.create_function(on),
        )
        .overload(LuaType::Function {
            params: vec![
                ("event", LuaType::Literal("audio")),
                (
                    "hook",
                    LuaType::Function {
                        params: vec![("event", LuaType::Class("AudioEvent"))],
                        returns: vec![],
                    },
                ),
            ],
            returns: vec![],
        })
        .overload(LuaType::Function {
            params: vec![
                ("event", LuaType::Literal("deliver")),
                (
                    "hook",
                    LuaType::Function {
                        params: vec![("event", schema::<DeliverEvent>())],
                        returns: vec![],
                    },
                ),
            ],
            returns: vec![],
        })
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
            ],
            returns: vec![],
        })
        .overload(LuaType::Function {
            params: vec![
                ("event", LuaType::Literal("record")),
                (
                    "handler",
                    LuaType::Function {
                        params: vec![("event", schema::<RecordEvent>())],
                        returns: vec![],
                    },
                ),
            ],
            returns: vec![],
        })
        .overload(LuaType::Function {
            params: vec![
                ("event", LuaType::Literal("session")),
                (
                    "handler",
                    LuaType::Function {
                        params: vec![("event", schema::<SessionEvent>())],
                        returns: vec![],
                    },
                ),
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
            "Start recording from the microphone. One recording runs at a time.",
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
            "Stop recording, if the core has not already, and return what was recorded.",
            vec![param(
                "focus",
                optional(schema::<Focus>()),
                "The window the text is for, such as a hotkey release's `focus`; observed now when absent.",
            )],
            vec![
                returns(
                    LuaType::Class("Audio"),
                    "audio",
                    "Empty when nothing was captured.",
                ),
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
            "capture.open",
            "Start capturing the microphone or what the computer plays, for as long as the plugin wants, alongside dictation. Read it with `Capture:read`; it closes when the plugin unloads.",
            vec![param("options", schema::<CaptureOptions>(), "")],
            vec![
                returns(optional(LuaType::Class("Capture")), "capture", ""),
                returns(optional(LuaType::String), "failure", "Why it could not start."),
            ],
            |lua| lua.create_async_function(capture_open),
        ),
        function(
            "Capture:read",
            "Wait until `seconds` of new audio are buffered, or the capture ends, and return everything buffered since the last read.",
            vec![param(
                "seconds",
                optional(LuaType::Number),
                "Default 1.",
            )],
            vec![
                returns(optional(LuaType::Class("Audio")), "audio", ""),
                returns(
                    optional(LuaType::String),
                    "ended",
                    "Why the capture ended, once everything it captured has been read.",
                ),
            ],
            |lua| lua.create_async_function(capture_read),
        ),
        function(
            "Capture:close",
            "Stop capturing and drop what has not been read.",
            vec![],
            vec![],
            |lua| lua.create_async_function(capture_close),
        ),
        function(
            "transcribe",
            "Run the `audio` hooks on `audio`, then the recognizer the `recognizer` setting names (default `local`), and return its text, trimmed.",
            vec![param("audio", LuaType::Class("Audio"), "")],
            vec![returns(LuaType::String, "", "Empty when nothing was said.")],
            |lua| {
                let prepare = lua.create_async_function(prepare_transcribe)?;
                lua.load(TRANSCRIBE)
                    .set_name("=dictate.transcribe")
                    .call(prepare)
            },
        ),
        function(
            "speech.decode",
            "Transcribe with the core's transcribe.cpp engine and the configured `model`. Silence, noise, and clips too short to hold speech give \"\". The bundled `recognizer` plugin registers this as the `local` recognizer.",
            vec![param("audio", LuaType::Class("Audio"), "")],
            vec![returns(LuaType::String, "", "")],
            |lua| lua.create_async_function(speech_decode),
        ),
        function(
            "recognizers.transform",
            "Register a transform of the recognizers registry. Every rebuild starts from an empty list and runs each transform once, in plugin order; add a recognizer to the list, or return a new list. A transform that errors, or leaves an entry without a string `id` and a `transcribe` function, disables its plugin. Only while the plugin file loads.",
            vec![param(
                "transform",
                LuaType::Function {
                    params: vec![("recognizers", array(LuaType::Class("Recognizer")))],
                    returns: vec![optional(array(LuaType::Class("Recognizer")))],
                },
                "",
            )],
            vec![],
            |lua| {
                let key = transforms_key(RECOGNIZERS);
                lua.create_function(move |lua, transform: Function| register(lua, &key, transform))
            },
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
            "data_dir",
            "This plugin's own directory for files it keeps, such as meeting transcripts; created if missing.",
            vec![],
            vec![returns(LuaType::String, "", "")],
            |lua| lua.create_function(data_dir),
        ),
        function(
            "ui.page",
            "Add a page to Dictate's window, listed in its sidebar under `title`. `render` runs each time the page is shown and returns what it shows; it runs to completion, so it may read files but not wait on commands or the core. A `render` that errors shows the error on the page. Only while the plugin file loads.",
            vec![
                param("id", LuaType::String, "Unique among pages."),
                param("title", LuaType::String, ""),
                param(
                    "render",
                    LuaType::Function {
                        params: vec![("query", schema::<PageQuery>())],
                        returns: vec![schema::<Feed>()],
                    },
                    "",
                ),
                param("options", optional(schema::<PageOptions>()), ""),
            ],
            vec![],
            |lua| lua.create_function(register_page),
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
            "Register a transform. Every rebuild starts from `config.toml` and runs each transform once, in plugin order, on a fresh copy; return a new list or edit it in place. A transform that errors, or returns something other than a list of entries, disables its plugin. Only while the plugin file loads.",
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
                    register(lua, &key, transform)
                })
            },
        ));
        functions.push(function(
            format!("{registry}.entries"),
            "The entries as of the last rebuild: `config.toml`'s, after every plugin's transform. While plugins load, the previous rebuild's, or `config.toml`'s at startup.",
            vec![],
            vec![returns(array(schema::<Entry>()), "", "")],
            move |lua| {
                lua.create_function(move |lua, ()| {
                    let entries = lua
                        .app_data_ref::<RegistryEntries>()
                        .and_then(|current| current.0.borrow().get(registry).cloned())
                        .unwrap_or_default();
                    lua.to_value(&entries)
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

fn on(lua: &Lua, (event, hook): (String, Function)) -> mlua::Result<()> {
    let key = match event.as_str() {
        "audio" => AUDIO_HOOKS,
        "transcript" => TRANSCRIPT_HOOKS,
        "deliver" => DELIVER_HOOKS,
        "hotkey" => HOTKEY_HANDLERS,
        "record" => RECORD_HANDLERS,
        "session" => SESSION_HANDLERS,
        _ => {
            return Err(mlua::Error::runtime(format!(
                "unknown event {event:?}; known events: \"audio\", \"transcript\", \"deliver\", \"hotkey\", \"record\", \"session\""
            )));
        }
    };
    register(lua, key, hook)
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

async fn session_stop(
    lua: Lua,
    (session, focus): (Table, Option<Value>),
) -> mlua::Result<(Audio, Value)> {
    const NAME: &str = "Session:stop";
    let session = session_id(&session)?;
    let focus = match focus {
        Some(focus) => Some(lua.from_value::<Focus>(focus)?.snapshot()?),
        None => None,
    };
    match core(&lua, NAME, CoreCall::SessionStop { session, focus })
        .await?
        .map_err(mlua::Error::runtime)?
    {
        CoreReply::Recording { audio, focus } => Ok((audio, focus_value(&lua, &focus)?)),
        other @ (CoreReply::Session { .. }
        | CoreReply::Capture { .. }
        | CoreReply::Chunk { .. }
        | CoreReply::Text { .. }
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
        | CoreReply::Capture { .. }
        | CoreReply::Chunk { .. }
        | CoreReply::Recording { .. }
        | CoreReply::Text { .. }
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
    let event = lua.to_value_with(
        &DeliverEvent {
            text,
            target: options.target,
            focus: options.focus,
        },
        json_to_lua(),
    )?;
    let Value::Table(event) = event else {
        return Err(mlua::Error::runtime("the deliver event is not a table"));
    };
    let event = hooks(&lua, NAME, DELIVER_HOOKS, event).await?;
    let DeliverEvent {
        text,
        target,
        focus,
    } = lua.from_value(Value::Table(event)).map_err(|error| {
        mlua::Error::runtime(format!("a deliver hook left a bad event: {error}"))
    })?;
    if text.is_empty() {
        core(&lua, NAME, CoreCall::OverlayHide)
            .await?
            .map_err(mlua::Error::runtime)?;
        let skipped = lua.create_string("a deliver hook skipped delivery")?;
        return Ok((Value::Nil, Value::String(skipped)));
    }
    let focus = focus.as_ref().map(Focus::snapshot).transpose()?;
    let call = CoreCall::DesktopDeliver {
        text,
        target,
        focus,
    };
    match core(&lua, NAME, call).await? {
        Ok(CoreReply::Delivered { target }) => Ok((lua.to_value(&target)?, Value::Nil)),
        Ok(other) => Err(unexpected(NAME, &other)),
        Err(failure) => Ok((Value::Nil, Value::String(lua.create_string(failure)?))),
    }
}

/// `dictate.capture.open`'s options.
#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct CaptureOptions {
    source: CaptureSource,
}

async fn capture_open(lua: Lua, options: Value) -> mlua::Result<(Value, Value)> {
    const NAME: &str = "dictate.capture.open";
    let CaptureOptions { source } = lua.from_value(options)?;
    match core(&lua, NAME, CoreCall::CaptureOpen { source }).await? {
        Ok(CoreReply::Capture { capture }) => {
            let fields = lua.create_table()?;
            fields.set("id", capture.0)?;
            fields.set("source", lua.to_value(&source)?)?;
            Ok((Value::Table(object(&lua, "Capture", fields)?), Value::Nil))
        }
        Ok(other) => Err(unexpected(NAME, &other)),
        Err(failure) => Ok((Value::Nil, Value::String(lua.create_string(failure)?))),
    }
}

fn capture_id(capture: &Table) -> mlua::Result<CaptureId> {
    Ok(CaptureId(capture.get("id")?))
}

async fn capture_read(
    lua: Lua,
    (capture, seconds): (Table, Option<f64>),
) -> mlua::Result<(Value, Value)> {
    const NAME: &str = "Capture:read";
    let seconds = seconds.unwrap_or(1.0);
    if !(0.0..=3600.0).contains(&seconds) {
        return Err(mlua::Error::runtime(format!(
            "{NAME}: seconds must be from 0 to 3600, not {seconds}"
        )));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // range checked above
    let samples = (seconds * f64::from(DICTATION_SAMPLE_RATE.as_hz())) as usize;
    let call = CoreCall::CaptureRead {
        capture: capture_id(&capture)?,
        samples,
    };
    match core(&lua, NAME, call).await? {
        Ok(CoreReply::Chunk { audio }) => {
            Ok((Value::UserData(lua.create_userdata(audio)?), Value::Nil))
        }
        Ok(other) => Err(unexpected(NAME, &other)),
        Err(ended) => Ok((Value::Nil, Value::String(lua.create_string(ended)?))),
    }
}

async fn capture_close(lua: Lua, capture: Table) -> mlua::Result<()> {
    let capture = capture_id(&capture)?;
    core(&lua, "Capture:close", CoreCall::CaptureClose { capture })
        .await?
        .map_err(mlua::Error::runtime)?;
    Ok(())
}

/// `dictate.transcribe`: the async part runs the `audio` hooks and picks
/// the recognizer; the recognizer then runs in the caller's coroutine, so
/// its own core calls park that coroutine.
const TRANSCRIBE: &str = r#"
local prepare = ...
return function(audio)
  local id, transcribe, prepared = prepare(audio)
  local ok, text = pcall(transcribe, prepared)
  if not ok then
    error(("recognizer %s failed: %s"):format(id, tostring(text)), 0)
  end
  if type(text) ~= "string" then
    error(("recognizer %s returned a %s, not a string"):format(id, type(text)), 0)
  end
  return (text:match("^%s*(.-)%s*$"))
end
"#;

async fn prepare_transcribe(
    lua: Lua,
    audio: AnyUserData,
) -> mlua::Result<(String, Function, AnyUserData)> {
    const NAME: &str = "dictate.transcribe";
    audio.borrow::<Audio>()?;
    let event = lua.create_table()?;
    event.set("audio", audio)?;
    let event = hooks(&lua, NAME, AUDIO_HOOKS, event).await?;
    let audio = event.get::<Value>("audio")?;
    let Some(audio) = audio
        .as_userdata()
        .filter(|audio| audio.is::<Audio>())
        .cloned()
    else {
        return Err(mlua::Error::runtime(format!(
            "an audio hook left event.audio as a {}, not audio",
            audio.type_name()
        )));
    };
    let selected = lua
        .app_data_ref::<SelectedRecognizer>()
        .map_or_else(|| "local".to_owned(), |selected| selected.0.clone());
    let mut registered = Vec::new();
    for entry in lua
        .named_registry_value::<Table>(RECOGNIZERS)?
        .sequence_values::<Table>()
    {
        let entry = entry?;
        let id: String = entry.get("id")?;
        if id == selected {
            return Ok((id, entry.get("transcribe")?, audio));
        }
        registered.push(id);
    }
    Err(mlua::Error::runtime(format!(
        "no recognizer {selected:?} is registered (registered: {registered:?}); set `recognizer` in config.toml or enable the plugin that provides it"
    )))
}

async fn speech_decode(lua: Lua, audio: AnyUserData) -> mlua::Result<String> {
    const NAME: &str = "dictate.speech.decode";
    let audio = audio.borrow::<Audio>()?.clone();
    match core(&lua, NAME, CoreCall::Decode { audio })
        .await?
        .map_err(mlua::Error::runtime)?
    {
        CoreReply::Text { text } => Ok(text),
        other @ (CoreReply::Session { .. }
        | CoreReply::Capture { .. }
        | CoreReply::Chunk { .. }
        | CoreReply::Recording { .. }
        | CoreReply::Focus { .. }
        | CoreReply::Delivered { .. }
        | CoreReply::Done) => Err(unexpected(NAME, &other)),
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
    let mode = lua
        .app_data_ref::<EffectiveContext>()
        .map_or(DictationMode::Message, |context| context.0.borrow().mode());
    let event = transcript_event(&lua, text, mode, options.focus)?;
    let event = hooks(&lua, NAME, TRANSCRIPT_HOOKS, event).await?;
    transcript_text(&event)
}

/// A new `transcript` event table.
fn transcript_event(
    lua: &Lua,
    text: String,
    mode: DictationMode,
    focus: Option<Focus>,
) -> mlua::Result<Table> {
    let event = TranscriptEvent {
        raw: text.clone(),
        text,
        mode,
        focus,
    };
    let Value::Table(event) = lua.to_value_with(&event, json_to_lua())? else {
        return Err(mlua::Error::runtime("the transcript event is not a table"));
    };
    Ok(event)
}

/// The text the `transcript` hooks left in `event`, trimmed.
fn transcript_text(event: &Table) -> mlua::Result<String> {
    let text = event.get::<Value>("text")?;
    if let Value::String(text) = &text {
        return Ok(text.to_string_lossy().trim().to_owned());
    }
    Err(mlua::Error::runtime(format!(
        "a transcript hook left event.text as a {}, not a string",
        text.type_name()
    )))
}

fn data_dir(lua: &Lua, (): ()) -> mlua::Result<String> {
    const NAME: &str = "dictate.data_dir";
    let plugin = active_plugin(lua)
        .ok_or_else(|| mlua::Error::runtime(format!("{NAME}: no plugin is running")))?;
    let dir = lua
        .app_data_ref::<DataDir>()
        .and_then(|dir| dir.0.clone())
        .ok_or_else(|| mlua::Error::runtime(format!("{NAME}: no data directory is set")))?
        .join(plugin);
    std::fs::create_dir_all(&dir)
        .map_err(|error| mlua::Error::runtime(format!("{NAME}: {}: {error}", dir.display())))?;
    Ok(dir.to_string_lossy().into_owned())
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
        for (field, ty, doc) in class.fields {
            lines.push(format!("---@field {field} {ty} {doc}"));
        }
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
    for signature in signatures {
        lines.push(String::new());
        lines.extend(signature);
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
        LuaType::Number => ("number".to_owned(), false),
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
