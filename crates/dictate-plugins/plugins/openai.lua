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
--   keywords = true                     # default: on for gpt-transcribe models
--   program = "curl"
--
-- The key is read from the environment when a recording is transcribed and
-- handed to curl on stdin, so it never appears in a process listing. A
-- server that needs no key works with key_env = "".
--
-- With keywords on, the written form of every [[dictionary]] entry, after
-- plugins' transforms, is sent as a keyword hint. Other models may reject
-- the field, so it is off for them unless keywords = true.

local options = ... or {}
local url = (options.url or "https://api.openai.com/v1"):gsub("/+$", "")
local key_env = options.key_env or "OPENAI_API_KEY"
local model = options.model or "gpt-transcribe"
local send_keywords = options.keywords
if send_keywords == nil then
  send_keywords = model:find("^gpt%-transcribe") ~= nil
end

-- The dictionary's written forms, once each, leaving out what the API
-- rejects: `<`, `>`, and line breaks.
local function keywords()
  local seen, list = {}, {}
  for _, entry in ipairs(dictate.dictionary.entries()) do
    local word = entry.written
    if word ~= "" and not word:find("[<>\r\n]") and not seen[word] then
      seen[word] = true
      table.insert(list, word)
    end
  end
  return list
end

local function transcribe(audio)
  local headers = ""
  if key_env ~= "" then
    local key = os.getenv(key_env)
    if not key or key == "" then
      error(("set %s to your API key, or key_env in [plugins.openai]"):format(key_env), 0)
    end
    headers = "Authorization: Bearer " .. key .. "\n"
  end
  local command = {
    options.program or "curl",
    "--silent", "--show-error", "--fail-with-body",
    "--header", "@-",
    "--form", "file=@" .. audio:wav() .. ";type=audio/wav",
    -- --form-string sends values as they are; --form would read a file for
    -- a value that starts with `@`.
    "--form-string", "model=" .. model,
  }
  if send_keywords then
    for _, word in ipairs(keywords()) do
      table.insert(command, "--form-string")
      table.insert(command, "keywords[]=" .. word)
    end
  end
  table.insert(command, url .. "/audio/transcriptions")
  local result, failure = dictate.exec(command, { stdin = headers, timeout_ms = options.timeout_ms or 60000 })
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
