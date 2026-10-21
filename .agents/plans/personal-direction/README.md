# Personal direction: my own Wispr Flow, on every machine I use

Direction set 2026-10-21 in conversation with Josh, after a day of
evaluating Underdog Bark 3.0 on the current sherpa-onnx stack. The
measurements behind every claim here are in
[memo-runtime-findings.md](memo-runtime-findings.md).

## The reframe

Dictate is a personal tool. There has been no release, nobody else is
using it, and the Linux gap that motivated it has since been filled by
other apps. That changes the goal:

- **Build it for me, on the three machines I hop between: macOS, Windows,
  and Linux.** Cross-platform is required, but "cross-platform" means
  three known machines, not arbitrary user hardware.
- **Replace Wispr Flow without the subscription.** Speed, good cleanup,
  context awareness, and polish are the bar. Paying monthly for something
  a little dev time can build is the thing being avoided.
- **Tailor it, and keep it a place to learn and try models.** No waiting
  on someone else's merge or maintaining a fork. Flailing a little is
  fine.
- **The models do the hard work.** The app is integration and taste:
  capture, delivery, overlay, formatting, hooks. That is what is worth
  owning.

## Inspirations

What each one contributes, and what Dictate does differently.

| Inspiration | What to take | What to leave |
|---|---|---|
| **Wispr Flow** | The bar: text appears right after you stop talking; cleanup that removes fillers and false starts; tone that adapts to the app; vocabulary learning; its move toward meetings | The subscription and cloud ASR |
| **Hex** ([anomalyco/hex](https://github.com/anomalyco/hex), Kit Langton; legacy Swift app at kitlangton/Hex) | The closest architectural reference. Rust + **GPUI** (same as Dictate) on Mac/Windows/Linux. Speech through **`transcribe-cpp`** with Q8_0 GGUF models, Metal on Mac, Vulkan on Windows/Linux, CPU fallback; streaming only on GPU. **No built-in LLM:** post-processing goes to a separately installed OpenCode service (stateless generate request with mode instructions, transcript, foreground app), then user TypeScript transforms from `~/.config/hex/hex.config.ts`. A "Voice Action" shortcut edits selected text. Falls back to the local transcript when OpenCode fails. The lesson to take is the boundary (no built-in LLM, hand off to an agent you already run), not OpenCode specifically | Its overlay, several UI choices, and its tie to a single agent (OpenCode). Its Linux support is beta and limited to i3/X11 and wlroots compositors; Dictate's Linux and Wayland integration (niri focus guards, layer-shell overlay, clipboard transactions) goes deeper |
| **Granola** | Meeting capture and transcripts as a first-class mode | Hex has an experimental meetings feature (Apple on-device summaries, Mac only); Dictate wants meetings on all three OSes |
| **Underdog / Bark 3.0** ([underdog.ai/bark](https://underdog.ai/bark), Conway Research, CC-BY-4.0) | **Bark 3.0 Clean** as the dictation model: Parakeet TDT 0.6B v3 post-trained to drop fillers, false starts, and repeats. It roughly halves cleanup error versus Parakeet v2 plus Dictate's formatter. Also Underdog's meeting labeling: "you" vs "the call" by audio source instead of full diarization | Their 163–180 MB `model.bark` format, which is tied to their Python/MLX loaders. Bark has no word timings yet |
| **Handy** | Its team builds `transcribe.cpp` ([handy-computer/transcribe.cpp](https://github.com/handy-computer/transcribe.cpp)), the runtime this direction adopts | Using Handy itself: fine, but not tailored |

## Direction

1. **Speech runtime: `transcribe-cpp` (ggml/GGUF) replaces sherpa-onnx**
   for final transcripts, behind a `Transcriber` seam in `dictate-speech`.
   - **Platforms:** one runtime on all three OSes (Metal, Vulkan, CUDA,
     ROCm, CPU).
   - **Models:** 16 families, including Parakeet TDT v2/v3,
     `parakeet-ultra`, Whisper, Moonshine (incl. streaming), SenseVoice,
     Qwen3-ASR, and diarization models.
   - **Memory:** quantized weights stay quantized in memory, mmapped on
     CPU and Metal.
   - **Why:** sherpa's prebuilt Rust libs are CPU-only, and onnxruntime's
     int8 rounds activations, which measurably hurt Bark (see memo). Hex
     reached the same runtime independently.
2. **The engine is pluggable, with per-platform defaults.** The model
   catalog picks a default per OS and GPU (for example, one model on Apple
   silicon/Metal, another on Linux/Vulkan with AMD). Trying a new model
   means adding a catalog entry, not changing the runtime.
3. **Bark 3.0 Clean is the target default model, pending measurement.**
   Get it into GGUF by unpacking `model.bark` into the stock NeMo
   Parakeet v3 checkpoint, then running transcribe.cpp's converter and
   `transcribe-quantize`. Q8_0 must match fp32 on the existing eval
   before it becomes a default.
4. **Batch first; drop sherpa's live partials.** Live text was added
   because sherpa offered it, and it isn't good. Revisit Realtime later
   with a streaming-native model on a GPU backend, which is what Hex does.
5. **A rock-solid core; features are Lua plugins.** Pi is the model:
   a small, dependable core that is easy to extend. When anyone can
   write personal software, extensibility is what makes one tool worth
   choosing over another.
   - **The core owns:** capture, recognition, the session state machine,
     the overlay states, and safe default primitives such as a
     focus-checked paste and clipboard transactions.
   - **Plugins are trusted and powerful.** They are not sandboxed or
     policed. They get the transcript, focused-app context, the core's
     primitives, HTTP, and process spawning, and they may bypass the
     primitives entirely. A buggy plugin is its author's choice.
   - **The core survives its plugins.** A plugin error, hang, or bad
     result never takes down the daemon or loses a transcript: errors
     come back as values, plugins run off the capture path, hooks have a
     time budget, and the local transcript is always the fallback.
   - **First-party features are plugins on the same API:** the formatter
     modes, post-processing, delivery targets, voice actions, and most of
     meetings. If the bundled features can be built as plugins, the API
     is good enough.
   - **The API comes from real plugins, not upfront design.** The
     formatter and the agent backends define it first.
   - **Language: Lua 5.4, embedded with `mlua`.** It compiles into the
     binary on all three OSes with no external runtime, starts fast, and
     can grow into an `init.lua` config where configuring and extending
     are the same thing. Dictate ships its own `dictate.*` Lua modules
     for hooks and platform access.
6. **No built-in LLM, and no single agent.** Post-processing is a set
   of bundled plugins with one request/response contract (mode
   instructions, transcript, foreground-app context in; text out), all
   optional and all falling back to the local transcript:
   - **OpenCode** (Hex's approach: its local service API).
   - **Pi** (the repo already ships `pi-extension/`).
   - **Codex app server**.
   - **A plain command hook** as the escape hatch for anything else.

   Choosing a backend is configuration, not a code change.
7. **Own the UI.** The overlay and settings stay Dictate's own design,
   not a Hex copy.
8. **Meetings become the second mode.** That means mic plus system audio,
   labeled by source, long-form chunking, and summaries through the
   same post-processing plugins.

## Sequence

Each row becomes a numbered, executor-ready plan when it is next up.
Plans are written one at a time against the live code, not all upfront.

| # | Title | Depends on | Status |
|---|---|---|---|
| 001 | Spike: Bark 3.0 Clean and Plus as GGUF (F16, Q8_0) on transcribe.cpp. Eval against the fp32 baseline in the memo, plus RAM and 2-thread latency | — | UNWRITTEN |
| 002 | `transcribe-cpp` behind a `Transcriber` seam. GGUF catalog with pinned revisions and hashes; `dictate transcribe` and fixture snapshots stay green | 001 informs defaults, not the seam | UNWRITTEN |
| 003 | Remove sherpa live partials and the streaming path (supersedes product-direction 009); remove sherpa once nothing uses it | 002 | UNWRITTEN |
| 004 | Backend and model defaults per platform (Metal / Vulkan / CUDA / CPU), with an override in settings | 002 | UNWRITTEN |
| 005 | Lua plugin core (`mlua`, Lua 5.4): pipeline hooks, `dictate.*` modules, error and time-budget containment. Proven by porting the formatter to a bundled plugin | 002 | UNWRITTEN |
| 006 | Agent post-processing plugins: OpenCode, Pi, Codex app server, command hook | 005 | UNWRITTEN |
| 007 | Meetings: per-OS system audio capture, source labels, long-form chunking, optional diarization, summaries through the post-processing plugins | 002, 006 | UNWRITTEN |
| 008 | Side quest: a Burn (pure Rust) backend behind the same seam | 002 | UNWRITTEN |

Status values: UNWRITTEN | TODO | IN PROGRESS | DONE | BLOCKED (one-line
reason) | SUPERSEDED (one-line pointer)

## Open questions

- Hardware on each machine: Apple silicon generation, GPU vendor on the
  Windows and Linux machines. This decides the per-platform defaults
  (004) and whether Realtime on GPU is worth revisiting.
- Does transcribe.cpp's Parakeet converter accept a `.nemo` carrying
  Bark's weights unchanged? Bark uses the stock Parakeet v3 architecture,
  so it should, but 001 must verify.
- Does ggml's Q8_0, which rounds activations per 32-value block, stay
  lossless on Bark? onnxruntime's per-tensor int8 did not.
- Meetings: is "you vs. the call" by audio source enough, or is real
  diarization needed? transcribe.cpp ships Sortformer and a multitalker
  Parakeet if it is.

- Plugin API shape, to settle in 005: which pipeline stages are hooks
  (transform text, choose delivery target, after delivery), how plugins
  are discovered and ordered, and whether settings move from TOML to
  `init.lua`.
- Post-processing integration details, to verify in 006: how Pi
  (RPC/SDK mode, or the existing `pi-extension`), the Codex app server,
  and OpenCode each accept a one-shot, stateless rewrite request, and
  what model selection each exposes.

## Considered and rejected

(So nobody re-runs these.)

- **Publishing Bark as sherpa-onnx int8 ONNX.** Measured lossy: int8
  Clean scored 6.46% vs 4.02% fp32 on cleaned DisfluencySpeech. The four
  `joshthomasdev/sherpa-onnx-nemo-underdog-bark-3.0-*-int8` Hugging Face
  repos are private (kept, not deleted). fp16 ONNX is lossless, but
  onnxruntime converts it to fp32 on load (~2.7 GB RAM).
- **Weight-only int8 ONNX (`MatMulNBits`).** Lossless, about 1.1 GB RAM,
  but about 3.4 s to decode 10 s of speech on 2 threads, versus about 1 s
  for the alternatives.
- **Staying on sherpa-onnx.** CPU-only prebuilt libraries, onnxruntime's
  int8 limits, and live partials that never worked well. This reverses
  product-direction's "second inference runtime" rejection: the current
  catalog can no longer supply what is wanted.
- **candle.** No Vulkan, so no AMD or Intel GPUs. Parakeet exists only
  in unverified community ports.
- **MLX.** Apple only.
- **TypeScript via Bun for plugins** (Hex's approach). Appealing, but it
  needs an external runtime installed on every machine. Lua embeds into
  the binary.
- **Sandboxing or policing plugins.** Plugins are trusted code the user
  chose to run. The core only guarantees its own survival.

## Deferred

- **Small Bark downloads (a Rust `model.bark` decoder).** Bark's
  163–180 MB file is about 2.2 bits per weight with entropy coding.
  `constriction` is a Rust crate, so a bit-exact decoder is feasible. With
  GGUF the download is about 700 MB (Q8_0), which removes the urgency.
  Revisit if download size starts to matter.
- **App-aware tone and vocabulary** (Wispr's per-app behavior). Hex's
  approach is to pass the foreground app to the agent prompt. Fold
  into 005 or follow it.
- **Luau instead of Lua 5.4.** Luau (Roblox's Lua) adds gradual types,
  compound assignment (`+=`), `continue`, if-expressions, string
  interpolation, and built-in sandboxing and interrupts. It drops 5.4's
  integers, bitwise operators, `goto`, `io`, and most of `os`.
  Plugins using either language's extras don't run on the other, so
  switching means rewriting some plugins; cheap while only first-party
  plugins exist. Revisit if typed plugins become appealing.
- **Realtime on GPU** with a streaming-native model (Parakeet Unified,
  Nemotron streaming, Moonshine streaming) after 003.
