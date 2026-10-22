-- Built-in agent cleanup: hands the formatted transcript to a coding agent
-- you already run, and uses its reply. Does nothing unless config.toml has
-- a [plugins.agent] table:
--
--   [plugins.agent]
--   backend = "pi"        # "pi", "codex", "opencode", or "command"
--   model = "anthropic/claude-haiku-4-5"   # optional; the agent's default otherwise
--   timeout_ms = 15000    # optional
--   program = "pi.cmd"    # optional; the executable to run (npm shims on Windows)
--   server = "http://127.0.0.1:4096"   # opencode only: use a running server's
--                                      # stateless generate API instead of `opencode run`
--   command = ["my-cleanup", "--flag"] # command only; gets JSON on stdin, prints the text
--   instructions = "..."  # optional; replaces the default instructions
--
-- If the agent fails, times out, or prints nothing, the text is kept.
-- Raw and literal modes are never sent: they mean "exactly what I said".

local options = ...
if type(options) ~= "table" or not options.backend then
  return
end

local STYLES = {
  message = "a chat message: casual and concise",
  email = "an email: complete sentences; add a greeting or sign-off only if one was spoken",
  note = "a personal note: terse is fine",
  technical = "technical writing: keep code identifiers, commands, and file names exact",
  command = "an instruction to an assistant: keep it imperative and exact",
}

local function instructions(ctx)
  if options.instructions then
    return options.instructions
  end
  local target = ""
  if ctx.app and (ctx.app.title or ctx.app.id) then
    target = " into " .. (ctx.app.title or ctx.app.id)
  end
  return table.concat({
    "Clean up this dictated text before it is inserted" .. target .. ".",
    "It is " .. STYLES[ctx.mode] .. ".",
    "Fix punctuation, capitalization, and obvious speech-recognition errors.",
    "Keep the speaker's words and meaning; do not add, answer, or summarize anything.",
    "Do not use tools or read files.",
    "Reply with only the final text, with no quotes or commentary.",
  }, " ")
end

local function with_model(command, flag)
  if options.model then
    table.insert(command, flag)
    table.insert(command, options.model)
  end
  return command
end

local function program(default)
  return options.program or default
end

-- Each backend returns the command to run, its stdin, and how to read the
-- reply from stdout.
local backends = {
  pi = function(prompt)
    return with_model({ program("pi"), "-p", "--no-session", "--no-tools" }, "--model"), prompt
  end,
  codex = function(prompt)
    local command = with_model({
      program("codex"), "exec", "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only",
    }, "--model")
    table.insert(command, "-")
    return command, prompt
  end,
  opencode = function(prompt)
    if not options.server then
      return with_model({ program("opencode"), "run" }, "--model"), prompt
    end
    local body = { prompt = prompt }
    if options.model then
      local provider, id = options.model:match("^([^/]+)/(.+)$")
      body.model = { providerID = provider, id = id }
    end
    local url = options.server:gsub("/+$", "") .. "/api/experimental/generate"
    local command = {
      program("curl"), "-fsS", "-H", "Content-Type: application/json", "--data-binary", "@-", url,
    }
    return command, dictate.json.encode(body), function(stdout)
      local reply = dictate.json.decode(stdout)
      return reply and reply.data and reply.data.text
    end
  end,
  command = function(prompt, text, ctx)
    local stdin = dictate.json.encode({
      text = text,
      raw = ctx.raw,
      mode = ctx.mode,
      app = ctx.app,
      instructions = instructions(ctx),
      prompt = prompt,
    })
    return options.command, stdin
  end,
}

local backend = backends[options.backend]
if not backend then
  error("unknown [plugins.agent] backend " .. tostring(options.backend)
    .. "; expected pi, codex, opencode, or command")
end
if options.backend == "command" and type(options.command) ~= "table" then
  error("[plugins.agent] backend = \"command\" needs command = [\"program\", \"args...\"]")
end

dictate.on("transcript", function(text, ctx)
  if not STYLES[ctx.mode] or text == "" then
    return nil
  end
  local prompt = instructions(ctx) .. "\n\n" .. text
  local command, stdin, read = backend(prompt, text, ctx)
  local result, failure = dictate.exec(command, { stdin = stdin, timeout_ms = options.timeout_ms or 15000 })
  if not result then
    dictate.log(failure or (options.backend .. " could not start; keeping the text"))
    return nil
  end
  if result.timed_out or result.code ~= 0 then
    dictate.log(("%s %s; keeping the text. %s"):format(
      options.backend,
      result.timed_out and "timed out" or ("exited with " .. tostring(result.code)),
      result.stderr:sub(1, 500)
    ))
    return nil
  end
  local reply = result.stdout
  if read then
    local ok, parsed = pcall(read, reply)
    reply = ok and parsed or nil
  end
  reply = reply and reply:match("^%s*(.-)%s*$")
  if not reply or reply == "" then
    dictate.log(options.backend .. " returned nothing; keeping the text")
    return nil
  end
  return reply
end)
