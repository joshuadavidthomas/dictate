---@meta dictate
-- Generated from crates/dictate-plugins/src/api.rs; do not edit.
-- `dictate plugins --types` installs this file next to your plugins.

---A focused window.
---@class dictate.App
---@field id? string Application identifier, such as `org.mozilla.firefox`.
---@field title? string Window title.

---`dictate.capture.open`'s options.
---@class dictate.CaptureOptions
---@field source dictate.CaptureSource

---Where a capture's audio comes from: `mic`, the microphone dictation
---uses, or `system`, what the computer plays, such as the other side of a
---call.
---@alias dictate.CaptureSource "mic"|"system"

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

---Audio from `dictate.capture.open`, recorded apart from dictation until closed.
---@class dictate.Capture
---@field id integer
---@field source dictate.CaptureSource
local Capture = {}

---A recording from `dictate.session.open`.
---@class dictate.Session
---@field id integer Identifies the session in `session` events.
local Session = {}

---Recorded samples, mono, from -1 to 1. `audio[i]` reads or writes sample `i` (from 1) and `#audio` is the number of samples.
---@class dictate.Audio
---@field rate integer Samples per second: 16000.
---@field [integer] number
---@field slice fun(self: dictate.Audio, from: integer, to?: integer): dictate.Audio A copy of samples `from` through `to` (default: the end).
---@field wav fun(self: dictate.Audio): string Write the audio to a 16 kHz, 16-bit mono WAV file and return its path, for engines that take a file. The file is deleted when the hook or handler that wrote it ends.
local Audio = {}

---A recording on its way through the `audio` hooks to the recognizer.
---@class dictate.AudioEvent
---@field audio dictate.Audio Edit its samples, or assign other audio, such as a slice.
local AudioEvent = {}

---A speech-to-text engine in the `recognizers` registry.
---@class dictate.Recognizer
---@field id string What the `recognizer` setting names.
---@field transcribe fun(audio: dictate.Audio): string The text heard in `audio`; empty when nothing was said.
local Recognizer = {}

---@class dictate
dictate = {}

dictate.session = {}
dictate.capture = {}
dictate.speech = {}
dictate.recognizers = {}
dictate.desktop = {}
dictate.overlay = {}
dictate.transcript = {}
dictate.text = {}
dictate.json = {}
dictate.dictionary = {}
dictate.replacements = {}

---Handle an event. `audio`, `transcript`, and `deliver` hooks run in plugin order on one event table: change its fields, and later hooks see the change. A hook that errors fails the `dictate.transcribe`, `dictate.transcript.run`, or `dictate.desktop.deliver` call that ran it. `hotkey`, `record`, and `session` handlers run in their own coroutines. Only while the plugin file loads.
---@param event "transcript"
---@param hook fun(event: dictate.TranscriptEvent)
---@overload fun(event: "audio", hook: fun(event: dictate.AudioEvent))
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

---Stop recording, if the core has not already, and return what was recorded.
---@param focus? dictate.Focus The window the text is for, such as a hotkey release's `focus`; observed now when absent.
---@return dictate.Audio audio Empty when nothing was captured.
---@return dictate.Focus focus The focused window when recording stopped.
function Session:stop(focus) end

---Stop recording and drop the audio.
function Session:cancel() end

---Start capturing the microphone or what the computer plays, for as long as the plugin wants, alongside dictation. Read it with `Capture:read`; it closes when the plugin unloads.
---@param options dictate.CaptureOptions
---@return dictate.Capture? capture
---@return string? failure Why it could not start.
function dictate.capture.open(options) end

---Wait until `seconds` of new audio are buffered, or the capture ends, and return everything buffered since the last read.
---@param seconds? number Default 1.
---@return dictate.Audio? audio
---@return string? ended Why the capture ended, once everything it captured has been read.
function Capture:read(seconds) end

---Stop capturing and drop what has not been read.
function Capture:close() end

---Run the `audio` hooks on `audio`, then the recognizer the `recognizer` setting names (default `local`), and return its text, trimmed.
---@param audio dictate.Audio
---@return string  Empty when nothing was said.
function dictate.transcribe(audio) end

---Transcribe with the core's transcribe.cpp engine and the configured `model`. Silence, noise, and clips too short to hold speech give "". The bundled `recognizer` plugin registers this as the `local` recognizer.
---@param audio dictate.Audio
---@return string
function dictate.speech.decode(audio) end

---Register a transform of the recognizers registry. Every rebuild starts from an empty list and runs each transform once, in plugin order; add a recognizer to the list, or return a new list. A transform that errors, or leaves an entry without a string `id` and a `transcribe` function, disables its plugin. Only while the plugin file loads.
---@param transform fun(recognizers: dictate.Recognizer[]): dictate.Recognizer[]?
function dictate.recognizers.transform(transform) end

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

---This plugin's own directory for files it keeps, such as meeting transcripts; created if missing.
---@return string
function dictate.data_dir() end

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

---The entries as of the last rebuild: `config.toml`'s, after every plugin's transform. While plugins load, the previous rebuild's, or `config.toml`'s at startup.
---@return dictate.Entry[]
function dictate.dictionary.entries() end

---Register a transform. Every rebuild starts from `config.toml` and runs each transform once, in plugin order, on a fresh copy; return a new list or edit it in place. A transform that errors, or returns something other than a list of entries, disables its plugin. Only while the plugin file loads.
---@param transform fun(entries: dictate.Entry[]): dictate.Entry[]?
function dictate.replacements.transform(transform) end

---The entries as of the last rebuild: `config.toml`'s, after every plugin's transform. While plugins load, the previous rebuild's, or `config.toml`'s at startup.
---@return dictate.Entry[]
function dictate.replacements.entries() end
