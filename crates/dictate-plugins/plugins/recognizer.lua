-- The local recognizer: the core's transcribe.cpp engine with the
-- configured `model`. Disable it with `[plugins.recognizer] enabled = false`
-- when another plugin provides the recognizer you use.
dictate.recognizers.transform(function(recognizers)
  table.insert(recognizers, { id = "local", transcribe = dictate.speech.decode })
end)
