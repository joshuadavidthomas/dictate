-- Dictation: push-to-talk on the `dictate` hotkey, and
-- `dictate record start|stop|toggle|cancel`. Records from the microphone,
-- transcribes the recording, runs the transcript hooks, and delivers it.
--
-- Replace it with your own `dictation.lua` in the plugins directory.

local session -- the open recording
local opening = false -- `dictate.session.open` has not answered yet
local queued -- a stop or cancel that arrived while opening
local target -- `--delivery` for this recording

local function finish(current, focus, to)
  local audio, stop_focus = current:stop(focus)
  local raw = #audio > 0 and dictate.transcribe(audio) or ""
  if raw == "" then
    return dictate.overlay.hide()
  end
  local text = dictate.transcript.run(raw, { focus = stop_focus })
  if text == "" then
    return dictate.overlay.hide()
  end
  dictate.desktop.deliver(text, { target = to, focus = stop_focus })
end

local function start(to)
  if session or opening then
    return
  end
  opening, target = true, to
  local opened = dictate.session.open()
  opening = false
  local pending = queued
  queued = nil
  if not opened then
    return
  end
  if not pending then
    session = opened
  elseif pending.cancel then
    opened:cancel()
  else
    -- Released or stopped before the recording had started.
    finish(opened, pending.focus, target)
  end
end

local function stop(focus, to)
  target = to or target
  if opening then
    queued = { focus = focus }
  elseif session then
    local current = session
    session = nil
    finish(current, focus, target)
  end
end

local function cancel()
  if opening then
    queued = { cancel = true }
  elseif session then
    local current = session
    session = nil
    current:cancel()
  end
end

dictate.on("hotkey", function(event)
  if event.name ~= "dictate" then
    return
  end
  if event.phase == "down" then
    start(nil)
  else
    stop(event.focus, nil)
  end
end)

dictate.on("record", function(event)
  if event.command == "start" then
    start(event.target)
  elseif event.command == "stop" then
    stop(event.focus, event.target)
  elseif event.command == "toggle" then
    if session or opening then
      stop(event.focus, event.target)
    else
      start(event.target)
    end
  elseif event.command == "cancel" then
    cancel()
  end
end)

dictate.on("session", function(event)
  if event.kind == "ended" and session and session.id == event.id then
    local current = session
    session = nil
    finish(current, nil, target)
  end
end)
