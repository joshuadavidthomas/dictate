-- Speech-to-text through an OpenAI-compatible API, as the `openai`
-- recognizer. Nothing is sent anywhere until config.toml selects it:
--
--   recognizer = "openai"
--
--   [plugins.openai]                    # all optional
--   model = "gpt-transcribe"
--   url = "https://api.openai.com/v1"   # or Groq, or a self-hosted server
--   key_env = "OPENAI_API_KEY"          # the environment variable with the key
--   timeout_ms = 60000
--   program = "curl"
--
-- The key is read from the environment when a recording is transcribed and
-- handed to curl on stdin, so it never appears in a process listing. A
-- server that needs no key works with key_env = "".

local options = ... or {}
local url = (options.url or "https://api.openai.com/v1"):gsub("/+$", "")
local key_env = options.key_env or "OPENAI_API_KEY"

local function transcribe(audio)
  local headers = ""
  if key_env ~= "" then
    local key = os.getenv(key_env)
    if not key or key == "" then
      error(("set %s to your API key, or key_env in [plugins.openai]"):format(key_env), 0)
    end
    headers = "Authorization: Bearer " .. key .. "\n"
  end
  local result, failure = dictate.exec({
    options.program or "curl",
    "--silent", "--show-error", "--fail-with-body",
    "--header", "@-",
    "--form", "file=@" .. audio:wav() .. ";type=audio/wav",
    "--form", "model=" .. (options.model or "gpt-transcribe"),
    url .. "/audio/transcriptions",
  }, { stdin = headers, timeout_ms = options.timeout_ms or 60000 })
  if not result then
    error(failure, 0)
  end
  if result.timed_out then
    error("the transcription request timed out", 0)
  end
  if result.code ~= 0 then
    error(("the transcription request failed (curl exit %s): %s %s"):format(
      tostring(result.code), result.stderr:sub(1, 300), result.stdout:sub(1, 300)), 0)
  end
  local ok, reply = pcall(dictate.json.decode, result.stdout)
  if not ok or type(reply) ~= "table" or type(reply.text) ~= "string" then
    error("the transcription reply has no text: " .. result.stdout:sub(1, 300), 0)
  end
  return reply.text
end

dictate.recognizers.transform(function(recognizers)
  table.insert(recognizers, { id = "openai", transcribe = transcribe })
end)
