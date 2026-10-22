# Plan 009: Core primitives, with dictation and meetings as plugins

> **Status: design.** This plan settles the plugin API and the
> daemon/plugin-host protocol before 006 builds agent plugins on them and
> before generated LuaLS types freeze names. Steps are executed one at a
> time against the live code; each step updates its row below.

## Why

Direction 5 says features are plugins and the core is small and solid.
Plan 005/006 built that for one narrow case: a text-in, text-out
`transcript` hook chain with per-hook budgets. Josh wants to push it as
far as it goes: push-to-talk dictation and meetings should be plugins
composed from core primitives, the way much of Neovim is Lua over a small
C core. If the bundled features can be written on the public API, the API
is good enough, and a dictation tool built as a kit is what distinguishes
Dictate from the many built as one feature.

## Where we stand (verified 2026-10-22)

The question was whether to lock plugins down (Roblox) or hand them a
curated API on an unrestricted runtime (Pi, OpenCode v2). Librarian
reads of each project's source:

| | Runtime | API handed to plugins | Timeouts | Cleanup |
|---|---|---|---|---|
| **Pi** | In-process jiti, full Node, "same operating-system permissions" | `pi` (~40 events, `registerTool`, `exec`) plus `ctx` objects | None on handlers; `pi.exec` has timeout and abort, bypassable | Plugin's own `session_shutdown`; reload tears down the runtime |
| **OpenCode v2** | In-process Bun, full runtime; v1's `$` shell helper removed | `Context` of domain facades, "does not expose OpenCode's private Core services" | None generic | Per-plugin Effect scope removes every hook and transform; a throwing transform disables only its plugin |
| **Roblox/Luau** | Locked down: no `io`/`package`, `os` time-only, read-only globals | Services via `game:GetService`, per-operation capability checks; `print` goes to the Output console | Wall-clock watchdog on the VM interrupt; one long C call still escapes | Engine-owned instances |

**Dictate is on the Pi/OpenCode side.** Plugins are code Josh writes or
chose to install, on his own machines; Roblox locks down because it runs
hostile code. So:

- The full Lua 5.4 standard library stays. No lockdown, no opt-in.
- Plugins get a curated `dictate.*` API that is the easy, correct way to
  do things (spawn a process the core can kill, log, deliver text). They
  may bypass it.
- The core keeps the guarantee neither Pi nor OpenCode makes: it survives
  its plugins. Plugins run in a separate process the daemon can kill, and
  dictation falls back to the local transcript.
- Ordinary plugin behavior must never break the host. Printing or
  shelling out is normal, so the protocol cannot share a channel with
  anything a plugin can write to (step 1: a private loopback socket for
  the protocol, the host's stdout to the log).

## The split: data plane in Rust, control plane in Lua

Audio frames, resampling, inference, keystroke injection, clipboard
transactions, overlay rendering, and per-OS system-audio capture stay in
the daemon. Moving 16 kHz samples through a pipe into Lua would add
latency and GC pauses for nothing, and the per-OS capture work is Rust
either way.

Plugins own the decisions: when to record and from which source, what to
do with the text, where it goes, and what follows (summaries, agents,
files). Plugins hold **handles** to core-owned resources, never the
resources themselves.

```
┌────────────────────────── daemon ──────────────────────────┐
│ hotkeys · audio sources · sessions · recognizer · overlay  │
│ desktop (focus, deliver) · processes · timers · HTTP       │
│ handle table: id → (plugin, resource)                      │
└──────────────┬──────────────────────────────▲──────────────┘
      events   │                              │  calls
   (hotkey,    ▼                              │  (session.open,
   segment,  ┌────────── plugin host ─────────┴─┐ deliver, exec)
   exit, …)  │ Lua VM, one coroutine per event  │
             │ handler; calls yield until reply │
             └──────────────────────────────────┘
```

## Protocol

Bidirectional, framed JSON over the private socket. Today it is
request/response from the daemon only; it becomes:

- **Events, daemon → host:** `{ "event": "hotkey", "name": "dictate", "phase": "down" }`,
  `{ "event": "session", "handle": 7, "kind": "segment", "text": "…", "start_ms": 0, "end_ms": 30000 }`.
  Each event runs the plugin's handler in a fresh coroutine.
- **Calls, host → daemon:** `{ "call": 12, "op": "session.open", "args": {…} }`
  answered by `{ "reply": 12, "ok": … }` or `{ "reply": 12, "error": "…" }`.
  In Lua, a call yields its coroutine with a value and resumes on the
  reply, so plugin code reads as straight-line code:
  `local text = session:stop()`.
- **Budgets measure Lua CPU time, not waiting.** The instruction hook's
  deadline covers each resume; time parked on a core call is bounded by
  that call's own `timeout_ms`.
  - A budget stop yields a private sentinel value that plugin code cannot
    produce; a core call yields its request. Any other yield (a plugin
    calling `coroutine.yield` itself) is an error, not a guess from "no
    values versus a table".
  - The deadline and active plugin are per coroutine, captured in each
    thread's hook closure. Today's single `Deadline` and `ActivePlugin`
    app-data slots only work because one call runs at a time; with
    parked coroutines they would be overwritten.
- **The daemon services calls on a thread that never waits on the host.**
  Today `wait_for` drops every line except the awaited reply. With host →
  daemon calls that is a deadlock: the host waits for a call's reply
  while the daemon's only reader waits for the result. One owner thread
  reads every message and dispatches it (replies to waiters, calls to
  handlers that answer asynchronously), with no shared lock.
- **The kill deadline counts host-responsible time.** The process
  deadline covers Lua CPU time plus slack, and pauses while the host is
  parked on a core call the daemon is serving (a `session:stop()` on a
  ten-minute segment is the daemon's time, not the plugin's).
- **Nothing waits on plugins on the capture path.** Today the mic worker
  thread calls the plugin chain and waits for the text. With hooks that
  call agents (006), a ten-second reply would keep push-to-talk from
  opening the mic. Plugin results come back as messages; the worker
  returns to listening as soon as the transcript is handed off.
  (Restarting the host after a plugin edit already runs off the worker
  thread.)
- **Handles are owned.** The daemon records which plugin opened each
  handle. When the host dies or restarts, the daemon closes every handle
  the host owned (kills processes, cancels timers). An open recording
  session is not discarded: the native flow finishes it (see below). When
  per-plugin reload exists, it closes only that plugin's handles: the
  same scoped cleanup OpenCode v2 gets from Effect scopes.
- **Types are generated.** Protocol and API types are Rust `serde` types;
  a generator emits the LuaLS `---@meta` file, and a test fails when the
  committed file is stale.

## Primitives (first cut)

Grown only as a real plugin needs them; this is the set the two sketches
below use.

| Module | Shape | Owner of the hard part |
|---|---|---|
| `dictate.on(event, fn, opts)` | events: `hotkey`, `transcript` (existing chain), `session`, `reload` | host |
| `dictate.hotkeys.register(name, default)` | named shortcuts; the platform binding is core | desktop crates |
| `dictate.session.open{ source, segment_ms?, keep_audio? }` | handle with `:stop() → text`, `:cancel()`, events `level`, `segment`, `closed` | dictate-speech |
| `dictate.audio.sources()` | mic devices now; `system` after 007's capture work | dictate-speech + desktop |
| `ctx.audio` in `dictate.on("transcript", fn, { audio = true })` | the finished utterance as a WAV path; see Post-utterance audio | daemon |
| `dictate.recognizers.transform(fn)` | register a plugin speech engine, chosen with `model = "plugin:<name>"` | host + daemon |
| `dictate.desktop.focus()` | app id, title, focus token for a guarded paste | dictate-desktop |
| `dictate.desktop.deliver(text, { target, focus })` | insert or clipboard with the existing focus guard and clipboard transaction | dictate-desktop |
| `dictate.overlay.show(state)` / `hide()` | the existing overlay states | dictate-ui |
| `dictate.text.format(text, ctx)` | the formatter (exists) | dictate-speech |
| `dictate.exec(argv, { timeout_ms, stdin, cwd })` | core-spawned, killed on timeout or unload, returns code/stdout/stderr | daemon |
| `dictate.http.request{ … }` | for agent backends with HTTP APIs | daemon |
| `dictate.timer.after(ms, fn)` / `every` | owned handles | host |
| `dictate.log(...)` | the daemon log (exists) | host |
| registries via `transform` | dictionary, replacements (exist); later modes, delivery targets, catalog | host |

## Sketches (the acceptance test for the API)

Push-to-talk dictation as the bundled `dictation.lua`:

```lua
local hotkey = dictate.hotkeys.register("dictate", "Super+Space")
local active

dictate.on("hotkey", function(event)
  if event.name ~= hotkey then return end
  if event.phase == "down" and not active then
    active = {
      focus = dictate.desktop.focus(),
      session = dictate.session.open({ source = "mic" }),
    }
    dictate.overlay.show("recording")
  elseif event.phase == "up" and active then
    local current = active
    active = nil
    dictate.overlay.show("transcribing")
    local raw = current.session:stop()
    if raw == "" then return dictate.overlay.hide() end
    local text = dictate.transcript.run(raw) -- the transcript hook chain
    dictate.desktop.deliver(text, { target = "insert", focus = current.focus })
  end
end)
```

Meetings as `meetings.lua`:

```lua
dictate.on("hotkey", function(event)
  if event.name ~= "meeting" or event.phase ~= "down" then return end
  local notes = {}
  local session = dictate.session.open({ source = { "mic", "system" }, segment_ms = 30000 })
  session:on("segment", function(segment)
    table.insert(notes, ("[%s] %s: %s"):format(segment.start, segment.source, segment.text))
  end)
  session:on("closed", function()
    local path = os.getenv("HOME") .. "/notes/" .. os.date("%Y-%m-%d-%H%M") .. ".md"
    local file = assert(io.open(path, "w"))
    file:write(table.concat(notes, "\n"))
    file:close()
    dictate.exec({ "pi", "-p", "Summarize the meeting in " .. path }, { timeout_ms = 120000 })
  end)
end)
```

If both read this cleanly against the implemented API, the API is right.
If a sketch needs a workaround, change the API, not the sketch.

## Performance rules (round-3 review, 2026-10-22)

Fable's perf review judged the design sound only as "policy is a
plugin". These are design rules, not later optimizations:

- **The core moves first on key-down.** It opens the mic (or starts
  keeping a pre-roll) the moment the key goes down, before asking the
  host, so the first syllable never waits on Lua or IPC.
- **Plugins claim a press within about 50 ms.** Past that, the native
  flow owns the press and a late claim is rejected.
- **One Lua VM per plugin.** One shared VM lets a CPU-heavy transcript
  hook in one plugin delay another plugin's hotkey handler. Separate VMs
  also make per-plugin reload clean (see Open questions).
- **Never through a plugin:** the audio callback, ring, and resampler;
  speech detection and segmentation compute; opening the mic; clipboard
  transaction mechanics; overlay rendering and spectrum.
- **Safe to move:** formatting, choosing the delivery target (bounded,
  with the configured target as fallback), agent calls (async), and the
  segmentation *policy* for meetings.
- **Do not lower the host's priority.** It is on the key-down path and
  would lose wakeups to ggml's spinning decode threads. Decode uses one
  thread per core but one instead, leaving a core free.
- The budget hook every 1,000 instructions costs about 1% at worst; not
  a concern.

Still open from the review: the mic opens per press (20–150 ms on
PipeWire). Keeping it open between presses would keep the OS mic
indicator on; opening on key-down with the core owning that step is the
plan for now. The clipboard settle checks (3 × 40 ms) and 180 ms paste
grace are untouched: they guard against duplicate or lost pastes and
need tuning on the real desktops, not in an orb.

## Post-utterance audio

Plugins never touch live samples, but they can have the finished audio.
This is the escape hatch for anything the rules above exclude from the
real-time path: trying a speech engine transcribe.cpp does not support, a
cloud API, offline noise suppression, saving recordings.

- **Opt in, per hook.** `dictate.on("transcript", fn, { audio = true })`
  gives that hook `ctx.audio = { path, sample_rate, duration_ms }`: a
  16 kHz mono WAV of the utterance. The daemon writes the file only when
  some hook asked for it (about 320 KB per 10 s; well under a
  millisecond).
- **Paths, not samples.** Only the path crosses the socket. The file is
  in a private per-user directory (0700), is deleted when the event
  finishes, and is never logged. A plugin that wants to keep it copies
  it.
- **Segments too.** A session opened with `keep_audio = true` puts the
  same `audio` table on each `segment` event, so a meetings plugin can
  archive or re-transcribe segments.
- **Plugin speech engines.** `dictate.recognizers.transform` registers a
  named engine: `fn(audio, ctx) → text`, with its own `timeout_ms`.
  `model = "plugin:<name>"` in config selects it, and the daemon then
  skips the built-in decode and calls the plugin with the WAV. If the
  plugin fails or times out, the daemon decodes locally with the default
  catalog model, so the local transcript stays the floor (at the cost of
  that one dictation's latency).
- **Headless.** `dictate transcribe <wav>` runs the same path, including
  plugin engines, so an engine plugin can be checked against the fixture
  corpus like any catalog model.

```lua
-- whisper-api.lua (sketch): a cloud engine as a plugin
dictate.recognizers.transform(function(engines)
  engines["whisper-api"] = function(audio)
    local result = dictate.exec({
      "curl", "-sf", "https://api.openai.com/v1/audio/transcriptions",
      "-H", "Authorization: Bearer " .. os.getenv("OPENAI_API_KEY"),
      "-F", "model=whisper-1", "-F", "response_format=text",
      "-F", "file=@" .. audio.path,
    }, { timeout_ms = 15000 })
    if result.code == 0 then return result.stdout end
  end
  return engines
end)
```

Not covered, by design: changing audio before the built-in decoder sees
it (a live noise suppressor or wake word). That would be a core stage
that plugins switch on and configure.

## Survival when dictation itself is a plugin

If the dictation flow lives in Lua and the host is dead, dictation is
gone, which breaks the core promise. So the daemon keeps today's flow as
a **native fallback**, and decides ownership once per hotkey press, at
`down`: if the host is not ready or does not answer the event in time,
the daemon owns that press and runs record → transcribe → local format →
guarded deliver itself. If the host dies after it opened a session, the
daemon adopts the session and finishes it natively, so the words already
spoken are delivered rather than lost. The bundled plugin is the normal path; the Rust
path is the floor. The fallback is the existing code, not a second
implementation to grow.

## Steps

| # | Step | Status |
|---|---|---|
| 1 | Private protocol channel (loopback socket, token handshake); host stdout and stderr go to the log, stdin is null; `PluginSetup` arrives over the channel instead of the host re-reading `config.toml` | DONE (2026-10-22: TCP loopback socket, since `std` has no portable way to hand a child an extra pipe; the address and a random token arrive on the child's stdin, which then closes. `std` sockets are close-on-exec / non-inheritable, so a plugin's own child processes never hold the protocol. Not built or run on Windows or macOS yet) |
| 2 | Core calls from plugins, with the time they wait excluded from the hook budget and the daemon's deadline; resources the daemon owns and cleans up when the host dies; plugin results never block the dictation worker | DONE (2026-10-22: plugins and delivery run on the delivery thread; `dictate.exec` is the first call. Rescoped: coroutine parking, one VM per plugin on its own thread, and the daemon-side call dispatcher only matter once events run concurrently, so they move to step 6 with hotkey events, where they can be tested. Calls today run synchronously inside the hook: the hook's deadline moves forward by the time spent waiting, and the host sends `exec_started`/`exec_exited` so the daemon extends its kill deadline by the command's timeout and kills the command's process group if it kills the host) |
| 3 | `dictate.exec` (done in step 2), `dictate.timer`, and `dictate.http` if 006 needs more than `curl` through `exec`; Windows Job Object so a kill reaches a plugin's children (today only the host dies) | PARTLY DONE (2026-10-22: no `dictate.http`: every agent backend works through `exec`, and OpenCode's stateless generate API through `curl`. Timers have no user until events exist and move to step 6. Added `dictate.json` and per-plugin options from `[plugins.<id>]`. Windows Job Object still open: no Windows toolchain in the orb to build or test it) |
| 4 | Generated LuaLS meta from the Rust types, with a staleness test | DONE (2026-10-22, not generated: the plugin API is mlua closures, not Rust types, and the protocol types never reach plugins. A hand-written `meta/dictate.lua` is valid Lua, and a test runs it and fails unless it defines exactly the functions the runtime `dictate` table has. `dictate plugins --types` installs it with a `.luarc.json`. Checked with lua-language-server 3.19.2 `--check` against the bundled plugins) |
| 5 | 006's agent plugins (OpenCode, Pi, Codex app server, command hook) on `exec`/`http` | DONE (2026-10-22: one bundled `agent.lua` with `pi`, `codex`, `opencode` (server generate API or `opencode run`), and `command` backends, configured by `[plugins.agent]`. Hooks get `ctx.app` from the stop-focus snapshot. Codex uses `codex exec`, not the app server: a one-shot process fits a per-utterance hook, and the app server is a long-lived JSON-RPC process that needs step 6's owned handles. Tested with fake agent executables; no real agent was installed in the orb) |
| 6 | Hotkey events and concurrency (coroutine per event, calls that park it, one VM per plugin on its own thread, a daemon reader that never waits on the host, per-coroutine budgets with a private yield marker); sessions, desktop, and overlay primitives; port push-to-talk to bundled `dictation.lua` with the native fallback; old hard-coded flow becomes the fallback only | TODO |
| 7 | Meetings on sessions (system audio per OS, source labels, segments) | TODO (is 007) |
| 8 | Post-utterance audio: `{ audio = true }` hooks with `ctx.audio`, `keep_audio` segments, `dictate.recognizers` and `model = "plugin:<name>"` with local fallback, same path in `dictate transcribe`. Can land any time after step 3 (needs `dictate.exec`) | TODO |

Steps 2–4 come before 006 because agent plugins need `exec` and `http`,
and the generated types should describe the bidirectional API, not the
current one-way chain. Step 6 comes after 006 so the API grows from the
agent plugins first, as direction 5 asks.

## Open questions

- Per-plugin hot reload: one VM per plugin is decided (see Performance
  rules), so reloading one plugin can drop only its VM and handles
  instead of restarting the whole host. Restarting is correct and
  cheap today; it stops being cheap once plugins hold long sessions, since
  a meeting would end when an unrelated plugin is edited.
- Hotkey ownership on Linux: global shortcuts go through the XDG portal,
  which wants shortcuts declared up front. Registration may need to
  happen at load, with changes requiring a restart.
- Whether `dictate.transcript.run` stays a separate hook chain or
  becomes ordinary plugin-to-plugin composition.
- macOS may show an Application Firewall prompt for the loopback
  listener on unsigned builds (reviewer's suspicion, unverified). Check
  on the Mac; if it does, switch Unix platforms to a Unix domain socket.

## Measured cost of the process split (2026-10-22, release build, orb CPU)

| | Time |
|---|---|
| Spawn host and load plugins | 11 ms |
| One transcript through the host (socket, JSON, thread hop) | 0.11 ms |
| Same chain in-process | 0.008 ms |
| Transcribing 10 s of speech, 2 CPU threads | ~1,000 ms |

The host uses about 21 MB resident (about 12 MB proportional, since most
pages are shared with the daemon's executable); the daemon holding Bark
Q8_0 uses about 1.2 GB. Process isolation is not the cost to watch;
blocking the worker or the single Lua VM is.
