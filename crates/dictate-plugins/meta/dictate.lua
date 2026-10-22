---@meta dictate
-- Type definitions for Dictate's plugin API, for lua-language-server.
-- `dictate plugins --types` installs this file next to your plugins.

---The focused window when recording stopped. Only known for insert delivery.
---@class dictate.App
---@field id? string Application identifier, such as `org.mozilla.firefox`.
---@field title? string Window title.

---What a transcript hook knows about the dictation.
---@class dictate.TranscriptContext
---@field raw string The recognizer's text, before any plugin ran.
---@field mode "raw"|"literal"|"message"|"email"|"note"|"technical"|"command"
---@field app? dictate.App

---@class dictate.HookOptions
---@field timeout_ms? integer Lua time this hook may take, 1 to 120000 (default 250). Time in `dictate.exec` does not count.

---One spoken-to-written pair in the dictionary or replacements.
---@class dictate.Entry
---@field spoken string
---@field written string

---@class dictate.ExecOptions
---@field timeout_ms? integer 1 to 120000 (default 10000). The command and everything it started are killed when it runs out.
---@field stdin? string Written to the command's stdin, which is then closed.
---@field cwd? string Working directory.

---@class dictate.ExecResult
---@field code? integer Exit code; nil when the command was killed.
---@field stdout string Up to 8 MiB.
---@field stderr string Up to 8 MiB.
---@field timed_out boolean

---@class dictate.Registry
local Registry = {}

---Register a transform. Every rebuild starts from `config.toml` and runs
---each transform once, in plugin order, on a fresh copy. Only while the
---plugin file loads.
---@param transform fun(entries: dictate.Entry[]): dictate.Entry[]?
function Registry.transform(transform) end

---@class dictate
dictate = {}

---Add a step to the per-utterance transcript chain. Return the new text, or
---nil to keep it. Only while the plugin file loads.
---@param event "transcript"
---@param hook fun(text: string, ctx: dictate.TranscriptContext): string?
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

---@type dictate.Registry
dictate.dictionary = Registry

---@type dictate.Registry
dictate.replacements = Registry

dictate.text = {}

---Run the built-in formatter with the current mode, dictionary, and
---replacements.
---@param text string
---@return string
function dictate.text.format(text) end

dictate.json = {}

---@param value any
---@return string
function dictate.json.encode(value) end

---JSON `null` decodes to nil.
---@param text string
---@return any
function dictate.json.decode(text) end
