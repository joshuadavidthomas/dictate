---@meta dictate
-- Generated from crates/dictate-plugins/src/api.rs; do not edit.
-- `dictate plugins --types` installs this file next to your plugins.

---A focused window.
---@class dictate.App
---@field id? string Application identifier, such as `org.mozilla.firefox`.
---@field title? string Window title.

---Options for `dictate.desktop.deliver`.
---@class dictate.DeliverOptions
---@field target? dictate.DeliveryTarget Where the text goes (default: the configured delivery).
---@field focus? dictate.Focus For `insert`: the window the text is meant for. When focus has moved, or no window is given, the text goes to the clipboard.

---Where delivered text goes.
---@alias dictate.DeliveryTarget "stdout"|"clipboard"|"insert"

---@alias dictate.DictationCommand "start"|"stop"|"toggle"|"cancel"

---How a transcript is formatted.
---@alias dictate.DictationMode "raw"|"literal"|"message"|"email"|"note"|"technical"|"command"

---One spoken-to-written pair in the dictionary or replacements.
---@class dictate.Entry
---@field spoken string
---@field written string

---Options for `dictate.exec`.
---@class dictate.ExecOptions
---@field timeout_ms? integer 1 to 120000 ms (default 10000). The command and everything it started are killed when it runs out.
---@field stdin? string Written to the command's stdin, which is then closed.
---@field cwd? string Working directory.

---What `dictate.exec` returns when the command ran.
---@class dictate.ExecResult
---@field code? integer Exit code; nil when the command was killed.
---@field stdout string Up to 8 MiB.
---@field stderr string Up to 8 MiB.
---@field timed_out boolean

---A window that had focus.
---@class dictate.Focus
---@field app? dictate.App The application, when the desktop reports it.
---@field token string Identifies the window for `dictate.desktop.deliver`'s focus check. Opaque.

---What a transcript hook knows about the dictation.
---@class dictate.HookContext
---@field raw string The recognizer's text, before any plugin ran.
---@field mode dictate.DictationMode
---@field app? dictate.App The window that had focus when recording stopped. Only known for insert delivery.

---Options for `dictate.on`.
---@class dictate.HookOptions
---@field timeout_ms? integer Lua time this hook may take, 1 to 120000 ms (default 250). Time spent in `dictate.exec` does not count.

---A hotkey press or release.
---@class dictate.HotkeyEvent
---@field name string The hotkey's name. `dictate` is the push-to-talk shortcut; `dictate hotkey <name> down|up` sends any name.
---@field phase dictate.HotkeyPhase
---@field focus? dictate.Focus The focused window when the key went up, when the core captured it.

---@alias dictate.HotkeyPhase "down"|"up"

---`dictate record <command>` from the command line or a compositor binding.
---@class dictate.RecordEvent
---@field command dictate.DictationCommand
---@field target? dictate.DeliveryTarget `--delivery`, when given.
---@field focus? dictate.Focus The focused window when the command was sent, when known.

---Options for `dictate.transcript.run`.
---@class dictate.RunOptions
---@field focus? dictate.Focus The window the text is for; hooks see its app as `ctx.app`.

---Something that happened to a session.
---@class dictate.SessionEvent
---@field id integer The session's `id`.
---@field kind dictate.SessionEventKind `ended`: the recording ended without `Session:stop`, for example at the length limit. `Session:stop` returns its transcript.

---@alias dictate.SessionEventKind "ended"

---A recording from `dictate.session.open`.
---@class dictate.Session
---@field id integer Identifies the session in `session` events.
local Session = {}

---@class dictate
dictate = {}

dictate.session = {}
dictate.desktop = {}
dictate.overlay = {}
dictate.transcript = {}
dictate.text = {}
dictate.json = {}
dictate.dictionary = {}
dictate.replacements = {}

---Handle an event. A `transcript` hook is a step in the per-utterance chain: return the new text, or nil to keep it. `hotkey`, `record`, and `session` handlers run in their own coroutines. Only while the plugin file loads.
---@param event "transcript"
---@param hook fun(text: string, ctx: dictate.HookContext): string?
---@param options? dictate.HookOptions
---@overload fun(event: "hotkey", handler: fun(event: dictate.HotkeyEvent), options: dictate.HookOptions?)
---@overload fun(event: "record", handler: fun(event: dictate.RecordEvent), options: dictate.HookOptions?)
---@overload fun(event: "session", handler: fun(event: dictate.SessionEvent), options: dictate.HookOptions?)
function dictate.on(event, hook, options) end

---Run a command to completion with a timeout Dictate enforces.
---@param command string[] Program and arguments; no shell.
---@param options? dictate.ExecOptions
---@return dictate.ExecResult? result
---@return string? failure Why the command could not start.
function dictate.exec(command, options) end

---Start recording from the microphone. One recording runs at a time.
---@return dictate.Session? session
---@return string? failure Why no recording could start.
function dictate.session.open() end

---Stop recording, if the core has not already, and wait for the transcript.
---@param focus? dictate.Focus The window the text is for, such as a hotkey release's `focus`; observed now when absent.
---@return string text Empty when nothing was heard.
---@return dictate.Focus focus The focused window when recording stopped.
function Session:stop(focus) end

---Stop recording and drop the audio.
function Session:cancel() end

---The window that has focus now.
---@return dictate.Focus
function dictate.desktop.focus() end

---Deliver text the way the core delivers dictation, including the focus check, clipboard transaction, and overlay.
---@param text string
---@param options? dictate.DeliverOptions
---@return dictate.DeliveryTarget? target Where the text went.
---@return string? failure
function dictate.desktop.deliver(text, options) end

---Hide the overlay, for example when a dictation ends without delivering anything.
function dictate.overlay.hide() end

---Run the transcript hook chain (the formatter and every `transcript` hook) on `text`.
---@param text string
---@param options? dictate.RunOptions
---@return string
function dictate.transcript.run(text, options) end

---Write a line to the daemon log.
---@param message string
function dictate.log(message) end

---Run the built-in formatter with the current mode, dictionary, and replacements.
---@param text string
---@return string
function dictate.text.format(text) end

---Encode a Lua value as JSON.
---@param value any
---@return string
function dictate.json.encode(value) end

---Decode JSON; `null` becomes nil.
---@param text string
---@return any
function dictate.json.decode(text) end

---Register a transform. Every rebuild starts from `config.toml` and runs each transform once, in plugin order, on a fresh copy; return a new list or edit it in place. Only while the plugin file loads.
---@param transform fun(entries: dictate.Entry[]): dictate.Entry[]?
function dictate.dictionary.transform(transform) end

---Register a transform. Every rebuild starts from `config.toml` and runs each transform once, in plugin order, on a fresh copy; return a new list or edit it in place. Only while the plugin file loads.
---@param transform fun(entries: dictate.Entry[]): dictate.Entry[]?
function dictate.replacements.transform(transform) end

