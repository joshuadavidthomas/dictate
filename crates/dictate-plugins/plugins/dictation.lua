-- Push-to-talk dictation. Dictate starts recording when the `dictate` hotkey
-- goes down; this plugin adopts the recording, then formats the transcript
-- through the transcript hooks and delivers it. If this plugin is missing,
-- broken, or too slow to claim a press, Dictate does the same itself.
--
-- Replace it with your own `dictation.lua` in the plugins directory.

local held = false
local session

local function finish(current)
  local raw, focus = current:stop()
  if raw == "" then
    return dictate.overlay.hide()
  end
  local text = dictate.transcript.run(raw, { focus = focus })
  if text == "" then
    return dictate.overlay.hide()
  end
  dictate.desktop.deliver(text, { focus = focus })
end

dictate.on("hotkey", function(event)
  if event.name ~= "dictate" then
    return
  end
  if event.phase == "down" then
    held = true
    local opened = dictate.session.open()
    if not opened then
      return -- Dictate keeps this press.
    end
    if held then
      session = opened
    else
      -- Released while the claim was in flight.
      finish(opened)
    end
  elseif session then
    held = false
    local current = session
    session = nil
    finish(current)
  else
    held = false
  end
end)
