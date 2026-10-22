# Plan 009: Everything is a plugin

> **Status: design, rewritten 2026-10-22.** The first version kept a native
> Rust dictation flow as a fallback, plus a process boundary, budgets, and
> kill deadlines so the core could "survive its plugins". This version
> drops that. Steps run one at a time against the live code, and each step
> updates its row below.

## Why

Direction 5 says features are plugins and the core is small. Push-to-talk,
meetings, the formatter, the speech engine, and delivery targets are all
plugins built on the public API. A bundled plugin is an ordinary plugin
that ships in the binary. Config can remove it, and a user file can
replace it.

The model is OpenCode v2 (`anomalyco/opencode` `dev` 52a7f4a, read from
source). It is the most widely used open-source coding harness, and it
works like this:

- Built-in plugins provide every tool, provider, VCS backend, agent, and
  config projection. So does every TUI and desktop feature outside the
  shell.
- Plugins get two primitives: `transform` (replayable, synchronous edits
  to a registry) and `hook` (ordered handlers sharing one mutable event).
- Plugins are in-process. There are no timeouts, no watchdog, and no
  native fallback. A plugin whose setup fails keeps its last good
  revision. A plugin whose transform throws is disabled, and its
  registries rebuild without it. A hook that throws fails that one
  operation.

The first version of this plan treated those absences as gaps. They are
choices. The extra machinery is code that every later change has to carry,
and it has not proven it is needed. A protection stays only after a test
shows the failure it prevents and shows that nothing cheaper prevents it.

## How Dictate differs from OpenCode

- **Dictate owns hardware.** The microphone is a device the OS grants to
  one process, and the OS shows an indicator while it is open. A plugin
  that fails while holding the mic must not leave it open. Of everything
  here, this is the one place that may need a protection beyond what
  OpenCode does, and it has to be proven (see "The microphone").
- **Dictate has no server/client split.** OpenCode's `ctx` is mostly the
  server's client API. Ours is the daemon's API: audio, recognizers,
  desktop, overlay.
- **The real-time path is short and hot.** A press has to reach an open
  mic in tens of milliseconds. In-process Lua calls cost microseconds, so
  this argues for keeping Lua in-process, not for keeping work out of Lua.

## What the core owns

The core is what plugins cannot replace, the same way OpenCode's session
runner is:

- The event loop and the plugin registry: load order, scopes, reload,
  disabling.
- Device mechanics: the capture thread, the ring buffer, resampling, and
  the platform hotkey bindings.
- Recognizer execution (ggml threads and model memory). Which engine runs
  is a plugin's choice.
- Desktop mechanics: focus snapshots, the clipboard transaction, and
  keystroke injection.
- Overlay rendering.

Everything else is a plugin: what a press does, which audio stages run,
which recognizer, which text stages, and where the text goes.

```
┌──────────────────────────── daemon ───────────────────────────┐
│ core: event loop · plugin registry · mic capture · ggml       │
│       desktop mechanics · overlay rendering                   │
│                                                               │
│ Lua VM (in-process), one coroutine per event                  │
│   dictate.dictation   press → session → audio hooks →         │
│                       recognizer → transcript hooks → deliver │
│   dictate.recognizer  registers the local engine              │
│   dictate.formatter   transcript hook                         │
│   dictate.agent       transcript hook                         │
│   user plugins        anything above, replaced or added       │
└───────────────────────────────────────────────────────────────┘
```

## Primitives

These are the same two as OpenCode, plus owned handles for devices.

- **`transform`**: a synchronous, replayable edit to a registry. A rebuild
  starts from the configured value and replays every active transform in
  plugin order. Registries: `hotkeys`, `recognizers`, `dictionary`,
  `replacements`, `modes`, and later `targets` (delivery targets, added
  when a second target exists). Registering a recognizer means adding it
  inside a `recognizers` transform. Dictionary and replacements already
  work this way.
- **`hook`**: handlers for one point, run in plugin order. Each gets one
  mutable event table, and later handlers see earlier changes.
  - `hotkey` `{ name, phase, focus }`
  - `audio` `{ audio }`, where `audio` is a sample buffer (below)
  - `transcript` `{ text, focus, mode }`; this replaces today's
    text-in, text-out chain
  - `deliver` `{ text, target, focus }`; setting `text = ""` skips
    delivery

  A hook that errors, or returns a value, fails that one dictation:
  later hooks do not run, nothing is delivered, the overlay shows that
  delivery failed, the log names the plugin, and the next press runs
  normally.
- **Handles**: core resources a plugin opens, owned by that plugin's
  scope, such as mic sessions and `dictate.exec` processes. Unloading or
  disabling the plugin closes everything it owns.
- **Calls** that wait (`session:stop()`, `dictate.exec`, recognizer
  runs) park the calling coroutine and resume it when the result is ready.
  The Lua thread is never blocked by a wait. This is the existing
  `PendingCall` runtime from step 6a.

## Plugin lifecycle

This follows OpenCode's activation model:

- **Ids.** A plugin's id is its file name: bundled `dictation`,
  `formatter`, `agent` (and `recognizer` in step 5), and user files by
  stem. A user file with a bundled plugin's name replaces it in its slot.
  (Changed in step 3 from `dictate.*` ids: dotted ids need quoted TOML
  keys, `[plugins."dictate.agent"]`.)
- **Config.** `[plugins.<id>]` with `enabled = false` turns a plugin off;
  its other keys are the plugin's options. Nothing is guarded. (Changed in
  step 3 from an OpenCode-style `plugins = ["-id", "*"]` list: TOML cannot
  have both a `plugins` array and `[plugins.<id>]` tables, and wildcards
  had no user.)
- **Boot order.** Bundled plugins, then user plugins, then config
  projections (dictionary, replacements, and modes from `config.toml`),
  so explicit config is applied last.
- **Scope.** Every transform, hook, and handle a plugin registers belongs
  to its scope. Closing the scope removes them all.
- **Reload.** When a file changes, plugins before the first changed one
  stay loaded. That plugin and every later one close in reverse order and
  load again in order, inside one batch, so each registry rebuilds once.
- **Failure.**
  - A load error keeps that plugin's last good revision running and shows
    the error. A failed revision is not retried until its file changes.
  - A transform that throws disables its plugin, and the registries
    rebuild without it.
  - A hook that throws fails that operation only.

  There is no native flow. With `dictate.dictation` disabled or broken, a
  press does nothing except show which plugin failed and why.

## The microphone

Starting assumption: it needs nothing beyond scope cleanup. The tests
below decide whether that holds:

1. **A plugin errors while recording.** The session handle's scope closes
   and the mic and the OS indicator turn off. Expected to hold through
   scope cleanup alone.
2. **A plugin loops forever while recording.** An in-process loop holds
   the Lua thread, so no Lua code can close the session. Measure what
   happens. If the mic stays open, the smallest fix is a core-side cap on
   session length, which already exists as auto-stop at the duration
   limit. Check that it closes the device without Lua. An instruction
   budget or a separate process comes back only if this fails.
3. **Press-to-open latency.** Measure key-down to mic-open with the press
   going through `dictate.dictation`, against today's core-first path. A
   pre-roll buffer in the core comes back only if the plugin path loses
   audible audio.
4. **Two sessions at once** (dictation during a meeting). The core opens
   the device once and gives each session its own copy of the samples.
   This is decided when meetings land (step 7), not before.

## Bundled dictation

```lua
local held, session

local function finish(current)
  local audio, focus = current:stop()
  local text = dictate.transcribe(audio)            -- audio hooks, then the recognizer
  text = dictate.transcript.run(text, { focus = focus }) -- transcript hooks
  if text ~= "" then dictate.desktop.deliver(text, { focus = focus }) end
end

dictate.on("hotkey", function(event)
  if event.name ~= "dictate" then return end
  if event.phase == "down" then
    held = true
    local opened = dictate.session.open({ source = "mic" })
    if held then session = opened else finish(opened) end
  elseif session then
    held = false
    local current = session
    session = nil
    finish(current)
  else
    held = false
  end
end)
```

`session:stop()` returns an audio buffer, not text. Transcription is a
separate call so that audio hooks and the recognizer registry sit between
the two.

## Audio in Lua

Boosting, silence trimming, saving recordings, and cloud engines are
plugin work. They run on the finished utterance, so no Lua runs inside
the audio callback.

- `audio` is userdata over the core's sample buffer: `#audio`,
  `audio[i]`, `audio[i] = x`, `audio:slice(a, b)`, `audio.rate`, and
  `audio:wav()`, which writes a temporary 16 kHz WAV and returns its path
  for engines that need a file.
- **Measure before restricting.** A Lua boost and a Lua silence trim over
  10 s of audio (160,000 samples) run through the `audio` hook with the
  real fixtures. If either adds noticeable time to a dictation, add the
  smallest helper that fixes it, such as `audio:map(fn)` in Rust. Do not
  add a native stage.
- Live, per-frame Lua (spectrum, speech detection) is out of scope until
  a plugin needs it. When one does, measure it the same way.

## Recognizers

- `dictate.recognizer` registers the local engine, the current
  transcribe.cpp path with the configured model, through
  `dictate.recognizers.transform`.
- `model = "<recognizer>:<model>"` picks the engine. Another plugin can
  add `whisper-api` or anything else, and disabling `dictate.recognizer`
  removes local transcription.
- `dictate transcribe <wav>` runs the same hooks and the same recognizer,
  so the fixture corpus checks plugin engines too.

## What gets deleted

These parts exist only for the fallback or for surviving plugins:

- `daemon/plugins.rs` ownership machinery: the 50 ms claim window,
  adoption, the 2 s stop grace, recording routing through `DeliveryJob`,
  `DaemonEvent::PluginRecord`, and native delivery of unclaimed jobs.
- The core starting a recording on key-down for the plugin to adopt.
- The plugin-host child process: the loopback socket, the token handshake,
  framed JSON, the writer thread, `PluginProcess`, `BlockingPlugins`, the
  `plugin-host` command, the process deadline, `exec_started` and
  `exec_exited`, and the planned Windows Job Object.
- Per-call instruction budgets (`LOAD_BUDGET`, `CALL_BUDGET`, the
  `timeout_ms` hook option, and the budget hook in `runtime.rs`), unless
  microphone test 2 proves they are needed.
- "A failing step keeps the text it was given" in the transcript chain. A
  failing hook fails the dictation and says so.

These stay: the coroutine runtime and `PendingCall`, `dictate.exec`
(owned by the plugin's scope, killed on unload), the generated LuaLS
types, and `dictate.json`.

## Steps

| # | Step | Status |
|---|---|---|
| 1 | **Move the host in-process.** One Lua VM on a daemon thread with the existing coroutine runtime. Core calls become Rust functions that park on the daemon's own work instead of socket messages. Delete the child process, socket, protocol framing, process deadline, and exec bookkeeping. Measure: hotkey event to handler, and a `transcript.run` round trip | DONE with step 2 (2026-10-22). The daemon's plugin thread owns `Plugins`, one Lua VM; core calls are served on that thread, and finished commands wake it through a callback. Deleted the child process, socket, handshake, protocol framing, process deadline, `exec_started`/`exec_exited`, and `dictate plugin-host`. Release build: loading the bundled plugins 0.58 ms (was 11 ms to spawn and load), hotkey down to `session.open` reaching the core 3.5 µs median, the transcript chain 4.7 µs (was 110 µs over the socket). Budgets stay until step 3 |
| 2 | **Delete the native flow.** Presses go only to `dictate.dictation`. Remove the claim window, adoption, stop grace, `PluginRecord`, recording-id routing, and native delivery. Add a headless audio source (a WAV played as the mic) so `dictate hotkey dictate down/up` runs the whole press without a real mic. Run microphone tests 1–3 | DONE (2026-10-22), done together with step 1, since porting the claim window in-process only to delete it was wasted work. Presses and `dictate record` commands reach plugins as `hotkey` and `record` events; a recording that ends on its own sends a `session` event; `Session:stop(focus)` carries the release's focus. Deleted the claim window, adoption, stop grace, native delivery, `ActiveRecordingDelivery` (`--delivery` now travels in the `record` event), and the push-to-talk recording matching. **The real-stack run found that the plugin path had never received a transcript**: the worker took the recording id from the mic, which closes at stop, so every transcript arrived without an id and the native fallback delivered it. The utterance now carries its recording id from the state machine. Not a WAV source: a PulseAudio null sink as the mic did the job without code. Verified in the orb with a release daemon, that virtual mic, and the LJ001-0001 fixture: hotkey press, `record toggle`, `record start`/`cancel`, a broken user `dictation.lua` (the press does nothing; the load error is logged), and recovery when it is deleted. Microphone tests 1–3 still open: they need the error and hang cases on a real mic, and the orb's null sink takes 0.8–1.2 s to open, which says nothing about real hardware |
| 3 | **Lifecycle.** Plugin ids, the config `plugins` list, per-plugin scopes over transforms, hooks, and handles, unchanged-prefix reload, last good revision on load failure, and disabling on transform failure. Remove the instruction budgets unless microphone test 2 needs them | DONE (2026-10-22). Plugins keep slots in load order. On reload, plugins before the first changed file stay loaded (their Lua state too); the rest are unloaded in reverse and loaded again in order. Changes are detected by file contents, not mtime and size. Unloading drops a plugin's registrations, cancels its open recording session (including one the core opens after the plugin is gone), kills the `dictate.exec` commands it waits on, and skips its hooks in transcript chains already running. A file that fails to load keeps its last good version running and is not rerun until it changes. A throwing transform disables its plugin and the registries rebuild without it. `[plugins.<id>] enabled = false` turns any plugin off; ids stay file names (see Plugin lifecycle). Reload no longer waits for the daemon to be idle. **Budgets deleted**: `LOAD_BUDGET`, `CALL_BUDGET`, hook `timeout_ms`, and the instruction hook. **Microphone tests, real daemon with a null-sink mic:** (1) a `dictation.lua` that opens a session then errors leaves the mic open, since handler errors do not unload; editing the file unloaded it and closed the mic 0.55 s later. (2) A handler stuck in `while true do end` after opening a session: the mic stayed open, then the core's 600 s limit closed it without Lua (press 14:56:07, closed by 15:06:39). The Lua thread kept spinning at 100% CPU, so plugins stay dead until a daemon restart, as in OpenCode. No budget or process boundary is needed for the mic. Test 3 (press-to-open latency) still needs real hardware. Eight mutations of the lifecycle code each fail a test |
| 4 | **Event hooks.** `transcript` becomes a mutable event (`formatter.lua` and `agent.lua` move with it), and `deliver` is added | DONE (2026-10-22). `transcript` and `deliver` hooks run in plugin order on one shared table: `{ text, raw, mode, focus }` after the formatter, and `{ text, target, focus }` inside `dictate.desktop.deliver`, where an empty `text` skips delivery. A hook that errors, returns a value, or leaves `text` as a non-string fails the whole chain with the plugin and point named. A handler that received a transcript and then fails is reported as a lost dictation, and the daemon shows delivery failed. `formatter.lua` and `agent.lua` set `event.text`; the agent leaves it unchanged when it fails. `dictate transcribe` exits non-zero on a hook failure. Deleted `TranscriptContext` and the text-in, text-out chain. Five mutations of the new paths each fail a test |
| 5 | **Audio and recognizers.** `session:stop()` returns audio, `audio` hook, `dictate.transcribe`, recognizers registry, bundled `dictate.recognizer`, and `dictate transcribe` on the same path. Measure Lua boost and trim on the fixtures | DONE (2026-10-22). The mic worker no longer decodes: it hands the plugin thread the samples, and `session:stop()` returns them as `Audio` userdata (`#a`, `a[i]`, `a[i] = x`, `a.rate`, `a:slice`). `dictate.transcribe(audio)` runs the `audio` hooks, then calls the recognizer the `recognizer` setting names (default `local`) in the caller's coroutine. `dictate.speech.decode` is the core's transcribe.cpp engine, served on a recognizer thread of its own; bundled `recognizer.lua` registers it as `local`. Selection is a `recognizer` setting rather than a `model` prefix, so `model` keeps meaning the local engine's model. `dictate transcribe` runs the same path, with decode served in process. **Measured** (release build, Lua audio hook): boost over every sample of LJ001-0001 (154,481 samples, 9.7 s) 37.9 ms, about 0.25 µs per sample; silence trim 0.4 ms; transcripts unchanged on three fixtures. That is small next to the decode, so no Rust helper. **Found an mlua 0.12.2 bug** (the latest release): when one coroutine catches a Rust callback's error with `pcall` while another waits in `yield_with`, the waiting one is resumed with a stray userdata in front of its values. The daemon already hit this whenever a transcript hook `pcall`ed a Rust error, for example `agent.lua`'s `pcall(read, reply)` when the reply is not JSON. Replies now reach parked coroutines through a per-coroutine slot instead. Real daemon with the null-sink mic: an audio hook trimmed 10,734 leading samples and the transcript was delivered; a user `echo` recognizer with the bundled one disabled delivered its text; `recognizer = "nope"` failed the dictation, named the registered recognizers, and released the mic. Follow-up the same day: `audio:wav()` writes a WAV into a 0700 temp directory and the file is deleted when the plugin call that wrote it ends; the recognizer thread loads the model at startup only when `recognizer = "local"`, else on first `dictate.speech.decode`, and a load failure fails that decode instead of marking dictation unavailable (the `Unavailable` phase is deleted). Real daemon: a `wavsize` recognizer with `model` set to an undownloaded model loaded and downloaded nothing, wrote a 310,388-byte WAV (44 + 2 × 155,172 samples), and the file was gone afterwards. Five mutations of the new paths each fail a test |
| 6 | Timers, `dictate.http`, and delivery-target registry, each when a plugin needs it | DONE for its first plugin (2026-10-22): bundled `openai.lua`, a recognizer for OpenAI-compatible transcription APIs, did not need `dictate.http`. `curl` through `dictate.exec` with `audio:wav()` does the request, the key travels on curl's stdin (`--header @-`) rather than its arguments, and the call parks like any other exec. Verified with real curl against a local stand-in for the API through `dictate transcribe`: the request carried the WAV, model, and key; a 401 failed the transcription with curl's exit and the body; a missing key named the variable to set; `model` pointed at an undownloaded 2.9 GB model stayed undownloaded. The default model is `gpt-transcribe`, OpenAI's recommended file-transcription model; `whisper-1` and the `gpt-4o-*-transcribe` models shut down on 2027-02-26. The plugin reads `text` from the default JSON reply rather than asking for `response_format=text`, which the docs do not promise for `gpt-transcribe`. `dictate.dictionary.entries()` and `dictate.replacements.entries()` return the rebuilt lists, and the plugin sends the dictionary's written forms as `keywords[]` (on by default for `gpt-transcribe` models only), once each, without values containing `<`, `>`, or line breaks, through `--form-string` so a leading `@` is text rather than a file. Real curl against a local stand-in: the body carried `GPUI` and `@/etc/passwd` as keywords and dropped `a<b`. **Real API** (key as a personal Amp secret): `dictate transcribe` with `recognizer = "openai"` took 1.0–1.7 s per fixture from the orb, and got "The Danger Trail, Philip Steele" where parakeet-110m heard "the Danger Trail, Philip Steels". A wrong key failed with the 401 body and exit 1. The API reads `keywords[]`: a value with `<` got the whole request rejected (`invalid_value`), so the filter is needed. Hints do not override clearly heard names ("Wytmoore" for Whitmore changed nothing), but on a TTS clip of project jargon they fixed what the model did not know: without the dictionary both runs heard "Miri" (and once "MLua", "ALua"); with it both runs gave "Niri" and "mlua". Timers and delivery targets still wait for a plugin that needs them |
| 7 | Meetings as a plugin on sessions (system audio per OS, source labels, segments, two sessions sharing the mic) | TODO (is 007) |

Each step ends with `just fmt`, `just test`, `just check`, and clippy. Run
`just hawk` after steps 1 and 3, which move interfaces.

## Done under the first version (2026-10-22, local commits)

These carry over unless the "What gets deleted" list removes them:

- Steps 1–2: a private loopback protocol, `dictate.exec`, and resources
  the daemon cleans up when the host dies. Step 1 of this plan replaces
  the process boundary.
- Step 3: `dictate.json` and per-plugin options from `[plugins.<id>]`.
  `dictate.http` was not needed, because agents work through `exec`.
- Step 4: LuaLS types generated from `api.rs`, with a staleness test
  (`DICTATE_UPDATE_LUA_TYPES=1`).
- Step 5: bundled `agent.lua` (pi, codex, opencode, command backends),
  tested with fake agent executables.
- Step 6 (`c298bbf`, `0150d98`, `911113d`):
  - Kept: the coroutine runtime with `PendingCall`, hotkey events,
    sessions, desktop and overlay calls, `dictate hotkey <name>
    down|up`, and bundled `dictation.lua`, including the quick-tap case
    the tests found.
  - Deleted by step 2: the native fallback and ownership machinery.
  - Not tested on real hardware.

Measured under the first version (release build, orb CPU): spawning the
host and loading plugins took 11 ms, and one transcript through the
socket took 0.11 ms against 0.008 ms in-process. Speed was never the
reason to drop the process boundary. The reason is the amount of code it
takes.

## Open questions

- Linux global shortcuts go through the XDG portal, which wants shortcuts
  declared up front. A `hotkeys` registry rebuild may need to re-bind
  through the portal, or ask for a restart.
- Whether `dictate.transcript.run` stays a core-dispatched hook chain or
  becomes plugin-to-plugin composition.
- What a stuck VM means for reload. An in-process loop that never yields
  holds the Lua thread, so a file edit cannot reload the plugin until the
  thread is free. OpenCode accepts the same risk. Microphone test 2 shows
  whether Dictate can too.
