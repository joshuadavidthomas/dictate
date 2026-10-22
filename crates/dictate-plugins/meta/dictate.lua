---@meta dictate
-- Generated from crates/dictate-plugins/src/api.rs; do not edit.
-- `dictate plugins --types` installs this file next to your plugins.

---A focused window.
---@class dictate.App
---@field id? string Application identifier, such as `org.mozilla.firefox`.
---@field title? string Window title.

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

---What a transcript hook knows about the dictation.
---@class dictate.HookContext
---@field raw string The recognizer's text, before any plugin ran.
---@field mode dictate.DictationMode
---@field app? dictate.App The window that had focus when recording stopped. Only known for insert delivery.

---Options for `dictate.on`.
---@class dictate.HookOptions
---@field timeout_ms? integer Lua time this hook may take, 1 to 120000 ms (default 250). Time spent in `dictate.exec` does not count.

---@class dictate
dictate = {}

dictate.text = {}
dictate.json = {}
dictate.dictionary = {}
dictate.replacements = {}

---Add a step to the per-utterance transcript chain. Return the new text, or nil to keep it. Only while the plugin file loads.
---@param event "transcript"
---@param hook fun(text: string, ctx: dictate.HookContext): string?
---@param options? dictate.HookOptions
function dictate.on(event, hook, options) end

---Run a command to completion with a timeout Dictate enforces.
---@param command string[] Program and arguments; no shell.
---@param options? dictate.ExecOptions
---@return dictate.ExecResult? result
---@return string? failure Why the command could not start.
function dictate.exec(command, options) end

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

