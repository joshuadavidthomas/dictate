-- History: every dictation, as heard and as typed, kept in this plugin's
-- data directory as one JSON line each (`history.jsonl`), and shown on a
-- History page in Dictate's window, newest first and grouped by day.
--
--   [plugins.history]
--   enabled = false          # keep nothing
--
-- A user plugin named history.lua replaces this file.

local function path()
  return dictate.data_dir() .. "/history.jsonl"
end

-- What the transcript hooks made of the dictation now being delivered.
local pending

dictate.on("transcript", function(event)
  pending = {
    raw = event.raw,
    mode = event.mode,
    app = event.focus and event.focus.app and (event.focus.app.title or event.focus.app.id),
  }
end)

-- Saved on delivery, so the text is what was typed after every hook.
dictate.on("deliver", function(event)
  local entry = pending
  pending = nil
  if not entry or event.text == "" then
    return
  end
  entry.at = os.time()
  entry.text = event.text
  -- Failing to save must not lose the dictation itself.
  local ok, failure = pcall(function()
    local file = assert(io.open(path(), "a"))
    file:write(dictate.json.encode(entry), "\n")
    file:close()
  end)
  if not ok then
    dictate.log("could not save the dictation: " .. tostring(failure))
  end
end)

local function entries()
  local list = {}
  local file = io.open(path(), "r")
  if not file then
    return list
  end
  for line in file:lines() do
    local ok, entry = pcall(dictate.json.decode, line)
    if ok and type(entry) == "table" and entry.text and entry.at then
      table.insert(list, entry)
    end
  end
  file:close()
  return list
end

local function day_title(at)
  local day = os.date("%Y-%m-%d", at)
  if day == os.date("%Y-%m-%d") then
    return "Today"
  elseif day == os.date("%Y-%m-%d", os.time() - 86400) then
    return "Yesterday"
  end
  return (os.date("%A, %B %d", at):gsub(" 0(%d)$", " %1"))
end

local function matches(entry, search)
  if not search or search == "" then
    return true
  end
  search = search:lower()
  return entry.text:lower():find(search, 1, true) or (entry.raw or ""):lower():find(search, 1, true)
end

dictate.ui.page("history", "History", function(query)
  local list = entries()
  local groups, current = {}, nil
  local shown = 0
  for index = #list, 1, -1 do
    local entry = list[index]
    if query.limit and shown >= query.limit then
      break
    end
    if matches(entry, query.search) then
      local title = day_title(entry.at)
      if not current or current.title ~= title then
        current = { title = title, rows = {} }
        table.insert(groups, current)
      end
      local time = os.date("%I:%M %p", entry.at):gsub("^0", "")
      table.insert(current.rows, {
        text = entry.text,
        -- What the recognizer heard, when formatting changed it.
        detail = entry.raw and entry.raw ~= entry.text and ("Heard \u{201c}" .. entry.raw .. "\u{201d}") or nil,
        meta = entry.app and (time .. " · " .. entry.app) or time,
      })
      shown = shown + 1
    end
  end
  local empty = "Nothing yet. Dictate something and it shows up here."
  if query.search and query.search ~= "" then
    empty = "Nothing matches \u{201c}" .. query.search .. "\u{201d}."
  end
  return { groups = groups, empty = empty }
end)
