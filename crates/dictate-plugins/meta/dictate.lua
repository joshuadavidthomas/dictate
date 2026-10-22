---@meta dictate
-- Generated from crates/dictate-plugins/src/api.rs; do not edit.
-- `dictate plugins --types` installs this file next to your plugins.

---A focused window.
---@class dictate.App
---@field id? string Application identifier, such as `org.mozilla.firefox`.
---@field title? string Window title.

---Text on its way through the `deliver` hooks to `dictate.desktop.deliver`.
---@class dictate.DeliverEvent
---@field text string The text to deliver. `""` skips delivery.
---@field target? dictate.DeliveryTarget Where it goes; nil means the configured delivery.
---@field focus? dictate.Focus For `insert`: the window the text is meant for.

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
---@field focus? dictate.Focus The window the text is for; hooks see it as `event.focus`.

---Something that happened to a session.
---@class dictate.SessionEvent
---@field id integer The session's `id`.
---@field kind dictate.SessionEventKind `ended`: the recording ended without `Session:stop`, for example at the length limit. `Session:stop` returns its transcript.

---@alias dictate.SessionEventKind "ended"

---A transcript on its way through the `transcript` hooks.
---@class dictate.TranscriptEvent
---@field text string The text so far. Assign to it to change the transcript; later hooks see the change. `""` means nothing is delivered.
---@field raw string The text `dictate.transcript.run` was given, before any hook ran.
---@field mode dictate.DictationMode
---@field focus? dictate.Focus The window the text is for, when known.

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

---Handle an event. `transcript` and `deliver` hooks run in plugin order on one event table: change its fields, and later hooks see the change. A hook that errors fails the `dictate.transcript.run` or `dictate.desktop.deliver` call that ran it. `hotkey`, `record`, and `session` handlers run in their own coroutines. Only while the plugin file loads.
---@param event "transcript"
---@param hook fun(event: dictate.TranscriptEvent)
---@overload fun(event: "deliver", hook: fun(event: dictate.DeliverEvent))
---@overload fun(event: "hotkey", handler: fun(event: dictate.HotkeyEvent))
---@overload fun(event: "record", handler: fun(event: dictate.RecordEvent))
---@overload fun(event: "session", handler: fun(event: dictate.SessionEvent))
function dictate.on(event, hook) end

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

---Register a transform. Every rebuild starts from `config.toml` and runs each transform once, in plugin order, on a fresh copy; return a new list or edit it in place. A transform that errors, or returns something other than a list of entries, disables its plugin. Only while the plugin file loads.
---@param transform fun(entries: dictate.Entry[]): dictate.Entry[]?
function dictate.dictionary.transform(transform) end

---Register a transform. Every rebuild starts from `config.toml` and runs each transform once, in plugin order, on a fresh copy; return a new list or edit it in place. A transform that errors, or returns something other than a list of entries, disables its plugin. Only while the plugin file loads.
---@param transform fun(entries: dictate.Entry[]): dictate.Entry[]?
function dictate.replacements.transform(transform) end

