-- Built-in formatter: spoken punctuation, filler removal, capitalization,
-- dictionary terms, and replacements for the configured mode.
--
-- A user plugin named formatter.lua replaces this file.
dictate.on("transcript", function(text)
  return dictate.text.format(text)
end)
