-- Meetings: `dictate hotkey meeting down` starts a meeting and stops it the
-- next time. While it runs, the microphone and what the computer plays are
-- captured apart from dictation, transcribed every `segment_seconds`, and
-- appended to a Markdown file in this plugin's data directory, labeled by
-- source: you on the microphone, the call on the computer's audio.
--
--   [plugins.meeting]        # all optional
--   segment_seconds = 15
--   silence = 0.01           # segments quieter than this peak are skipped
--   you = "You"
--   call = "Call"
--
-- Wear headphones: through speakers the microphone hears the call too, and
-- its words appear twice. Dictation keeps working during a meeting.

local options = ... or {}
local segment_seconds = options.segment_seconds or 15
local silence = options.silence or 0.01
local labels = { mic = options.you or "You", system = options.call or "Call" }

local meeting -- the meeting running now

local function append(path, text)
  local file = assert(io.open(path, "a"))
  file:write(text)
  file:close()
end

-- Whether any sample, checked every millisecond, is louder than
-- `silence`: quiet segments are not sent to the recognizer, which may be a
-- paid API.
local function heard(audio)
  for index = 1, #audio, audio.rate // 1000 do
    local sample = audio[index]
    if sample > silence or sample < -silence then
      return true
    end
  end
  return false
end

local function clock(seconds)
  seconds = math.floor(seconds)
  return string.format("%02d:%02d:%02d", seconds // 3600, seconds // 60 % 60, seconds % 60)
end

-- One segment of `source`: transcribed and written, or nothing when
-- silent. Returns false once the capture has ended.
local function segment(current, source, seconds)
  local audio, ended = source.capture:read(seconds)
  if not audio then
    dictate.log(labels[source.capture.source] .. " audio ended: " .. ended)
    return false
  end
  local at = source.read
  source.read = source.read + #audio / audio.rate
  if not heard(audio) then
    return true
  end
  local ok, text = pcall(dictate.transcribe, audio)
  if not ok then
    dictate.log("a segment could not be transcribed: " .. tostring(text))
  elseif text ~= "" then
    append(current.path, string.format("**%s** [%s]: %s\n\n", labels[source.capture.source], clock(at), text))
  end
  return true
end

local function run(current)
  for _, kind in ipairs({ "mic", "system" }) do
    local capture, failure = dictate.capture.open({ source = kind })
    if capture then
      table.insert(current.captures, capture)
      table.insert(current.sources, { capture = capture, read = 0 })
    else
      dictate.log("recording the meeting without " .. labels[kind] .. ": " .. failure)
    end
  end
  if #current.sources == 0 then
    meeting = nil
    return
  end
  append(current.path, "# Meeting " .. os.date("%Y-%m-%d %H:%M") .. "\n\n")
  dictate.log("recording a meeting to " .. current.path)

  local last = false
  repeat
    -- Once stopping, read what is left without waiting.
    last = current.stopping
    local seconds = last and 0 or segment_seconds
    local live = {}
    for _, source in ipairs(current.sources) do
      if segment(current, source, seconds) then
        table.insert(live, source)
      end
    end
    current.sources = live
  until last or #current.sources == 0

  for _, capture in ipairs(current.captures) do
    capture:close()
  end
  meeting = nil
  dictate.log("the meeting is saved to " .. current.path)
end

dictate.on("hotkey", function(event)
  if event.name ~= "meeting" or event.phase ~= "down" then
    return
  end
  if meeting then
    meeting.stopping = true
    return
  end
  meeting = {
    path = dictate.data_dir() .. "/" .. os.date("%Y-%m-%d-%H%M%S") .. ".md",
    captures = {},
    sources = {}, -- the captures still being read
    stopping = false,
  }
  run(meeting)
end)
