# Product direction: make Dictate undeniably good

Direction work from the 2026-06-11 `/improve next` audit, planned at
revision `pkzmprvzlnsn` (git `e65b4661cfcf`; source tree identical to
`dd6db2c175a3`). Sourced from a competitive survey of the 2026 dictation
landscape (macOS: Wispr Flow, Aqua Voice, Superwhisper, MacWhisper,
VoiceInk; Linux: Handy, Voxtype, OpenWhispr, hyprvoice, Speech Note, et
al.), Wayland feasibility research, and local ASR/LLM state-of-the-art —
key findings are embedded in each plan's "Why this matters". The thesis:
the gap between Dictate and the loved macOS apps is, in order, **delivery**
(text must land where the user works), **configurability** (the formatter
and model catalog exist but are unreachable), **model quality** (Parakeet
has displaced Whisper as the local default everywhere), and **legible
overlay states**. No Linux app does all four well; that is the opening.

**Cross-effort dependency**: `.agents/plans/gpui-rewrite-hardening/` touches the
same files (`src/daemon.rs`, `src/mic.rs`, `src/overlay.rs`). Land that
effort first — plan 004 here hard-depends on hardening 004, plan 005 here
hard-depends on hardening 003/005/006. Never run the two efforts
concurrently.

Execute in the order below unless dependencies say otherwise. Each
executor: read the plan fully before starting, honor its STOP conditions,
and update your row when done.

## Execution order & status

| Plan | Title | Effort | Depends on | Status |
|------|-------|--------|------------|--------|
| [001](001-clipboard-delivery.md) | Clipboard delivery through a typed delivery seam | S–M | hardening track landed | DONE |
| [002](002-insertion-spike.md) | Spike: pick the Wayland text-insertion mechanism | M | — (parallel-safe; examples only) | DONE |
| [003](003-settings-foundation.md) | TOML settings unlock the formatter and model catalog | M | 001 | DONE |
| [004](004-default-model-parakeet.md) | Evaluate Parakeet default; retire the 30s ceiling | S–M | hardening 004 | DONE (re-run 2026-07-05 after formatter-punctuation-compat landed; default flipped to parakeet-tdt-0.6b-v2-int8, cap raised to 10 min) |
| [005](005-overlay-phase-states.md) | Overlay recording/transcribing/error states | M | hardening 003, 005, 006 | DONE (2026-08-13 audit: implemented beyond plan scope — `OverlayState` has six states incl. delivery outcomes, distinct visuals per state in `crates/dictate-ui/src/overlay.rs`, daemon drives all transition sites) |
| [006](006-live-partials-spike.md) | Spike: live partials without leaving sherpa-onnx | S–M | 004 | SUPERSEDED (2026-08-07: the daemon now feeds live partials through a streaming model — `partials_model`, default `fast-conformer-ctc-en-80ms-int8` — while `model` keeps producing the final text; see the streaming transcription change) |
| [007](007-live-partials-surface.md) | Live partials text surface above the overlay pill | M | streaming partials (landed), 005 | DONE (2026-08-13: live verified on niri; refined after use to GPUI-native wrapping, 1–4-line growth, bottom scrolling, overflow indicator, and configurable font) |
| [008](008-audio-ducking.md) | Duck system audio while recording | M | — (implemented alongside 007) | DONE (2026-08-13: disposable-sink matrix verified normal/cancel/user-change/default-switch/disabled/crash/duplicate-daemon paths) |
| [009](009-batch-realtime-recognition.md) | Separate Batch and Realtime recognition pipelines | L | 007, 008 | TODO (executor-ready; refreshed after 008 completion) |

Status values: TODO | IN PROGRESS | DONE | BLOCKED (one-line reason) |
SUPERSEDED (one-line pointer to what replaced it)

## Dependency notes

- **001 → 003**: settings absorb 001's `--delivery` flag as persistent
  config (flag stays as runtime override).
- **002 ∥ everything**: the spike writes only `examples/`, dev-deps, and a
  findings doc. Its *follow-up* (an `Insert` delivery target) gets planned
  from `spike-insertion-findings.md` after the maintainer reads the verdict.
- **004 after hardening 004**: it amends the recording cap and README note
  that plan creates.
- **005 last**: phase polish is wasted until hardening 006 makes the
  overlay smooth, and it extends call sites hardening 003/005 reorder.
- **004 → 006**: the partials spike re-decodes with the Parakeet catalog
  entries; 004's eval is what proves they work end-to-end here. Like 002,
  006 writes only `examples/` + a findings doc, so it is otherwise
  parallel-safe. Its positive verdict spawned the live surface and mixed
  daemon pipeline that 007 made visible.
- **007 + 008 → 009**: live use exposed the flaw in treating one
  streaming recognizer as a preview of an unrelated Batch recognizer.
  009 keeps the surface for Realtime and gives Batch and Realtime one
  recognizer each. Its baseline now includes 008's completed daemon
  lifecycle and startup-recovery ordering.

## Reconciliation log

- **2026-08-13 (Batch/Realtime decision)**: Added 009 after live use of
  007 showed sharp wording differences between Fast Conformer hypotheses
  and offline Parakeet delivery. Settled two recognition modes: Batch is
  the default and has no live transcript; Realtime uses one streaming
  session for both hypotheses and delivered text. Marked 007 DONE after
  niri verification and UI refinement. Marked 008 IN PROGRESS: its core
  works after fixing Pulse context/main-loop drop order. Finished 008
  with disposable sinks: exact normal/cancel restore, user adjustment,
  default-sink switch, disabled mode, SIGKILL recovery, ducking-only
  failure, and duplicate-daemon ownership all behaved as designed.
  Recovery now runs after socket ownership, so a second daemon cannot
  heal an active first daemon's duck. Ambiguous asynchronous volume
  updates retain both guard and disk recovery until the sink state is
  reconciled. Refreshed 009 against that source.
- **2026-08-13 (later still)**: Rewrote 007 for execution against main
  `0165f013`. The original draft's revision-only Partial guard would
  reopen the text card after Transcribing Show / Hide because the worker
  can still `Keep` and feed. The rewrite pins `send_partial` as
  revision-load-only, adds an apply table that accepts Partial only while
  the pill is Recording, and names the fixed 420×72 / 100px-margin
  second surface. Still TODO; next executable plan.
- **2026-08-13 (later)**: Added 008 (duck system audio while recording)
  from maintainer request: slight, adjustable dip of the default sink
  while recording is live — adjustable because calls and music want
  different amounts; `duck_audio = 0` disables. Flat sink dip chosen
  over role-aware ducking; restore discipline (every exit path, user
  adjustments, crash recovery) is the heart of the plan.
- **2026-08-13**: Audited 005 against the live code: implemented beyond
  plan scope (six `OverlayState`s including delivery outcomes, distinct
  visuals, daemon-driven transitions) — marked DONE. Added 007 (live
  partials text surface) as the follow-up the superseded 006 promised;
  maintainer decision: the text surface is separate from the spectrum
  pill, positioned above it.
- **2026-06-11 (later)**: Added 006 after maintainer discussion of the
  streaming trade-off: live partials don't need a streaming model here
  because final text always comes from the offline decode at stop —
  partials are overlay cosmetics. 006 spikes periodic Parakeet re-decode
  (Superwhisper's "realtime" trick) vs a two-pass hybrid, both inside
  sherpa-onnx. The "considered and rejected" streaming entry was rewritten
  to scope the rejection to second inference runtimes only.
- **2026-06-11**: Effort created from the `/improve next` direction audit
  (competitive survey + Wayland/ASR feasibility research, four
  web-research passes). Five plans; 002's output is a findings doc that
  seeds a future insertion-implementation plan. Next: finish
  gpui-rewrite-hardening, then 001.

## Considered and rejected

(So nobody re-audits these.)

- **A second inference runtime for true streaming** (Kyutai STT via
  candle, or hand-rolled Moonshine v2 streaming on `ort`): sherpa-onnx's
  online API serves only Zipformer/Paraformer/older-FastConformer models
  (an accuracy step down); Parakeet has no streaming export and Moonshine
  v2 — streaming by architecture — is wrapped offline-only in sherpa-onnx
  (k2-fsa docs, confirmed 2026-06). Cobbling a second runtime means a
  second model-catalog family, a heavy new dependency, and (for Kyutai)
  unproven CPU performance. Re-open the runtime question when the current
  catalog cannot supply an acceptable Realtime model.
- **One recognizer for live text and another for delivered text**: live
  use showed that unrelated hypotheses make the preview misleading. 009
  removes this mixed pipeline. A future streaming-plus-rescore design
  needs one model family built for that contract and its own named mode.
- **xdg-desktop-portal GlobalShortcuts**: not implemented on niri
  (niri discussion #2775) or Sway (xdg-desktop-portal-wlr #240); the
  current compositor-keybind → `dictate record toggle` socket approach
  already works on every compositor with zero consent dialogs. Revisit
  for Flatpak packaging or when niri lands portal support.
- **ydotool/uinput as the primary insertion mechanism**: the setup
  friction (daemon, `input` group, distro-specific socket paths) is the
  single biggest complaint cluster across every surveyed Linux dictation
  tool. It may earn a place as an opt-in fallback — plan 002's spike
  decides — but never as the default path.
- **Caret-adjacent overlay placement**: infeasible cross-compositor — the
  IM popup-surface route conflicts with real input methods, and
  `text-cursor-position` has zero compositor implementations. Fixed
  bottom-anchored layer-shell is the production consensus (and what Wispr
  Flow effectively does at bottom-center on macOS). Already Dictate's shape.
- **`enigo` as the injection crate**: its portal/libei session dies
  silently after lock/sleep/compositor restart (Handy PR #1395's whole
  reason to exist). Plan 002 assesses alternatives directly.
- **Cloud ASR / proprietary-model envy (Aqua Avalon, Wispr cloud)**:
  local-first is Dictate's identity and the lifetime-license/local apps'
  differentiator per the survey; cloud ASR is out. (LLM cleanup BYOK is a
  separate, deferred question — see below.)

## Deferred

(Real direction findings, not planned in this effort.)

- **App-aware profiles** (PLAN.md:241-243 names this; Superwhisper/Wispr's
  "magic" feature): needs settings (003) as the profile substrate plus
  per-compositor focused-window IPC (`niri-ipc` `FocusedWindow` on niri;
  no portable mechanism — GNOME needs a Shell extension). Design after 003
  lands; the natural config shape is noted in 003's maintenance notes.
- **Optional LLM rewrite stage** (PLAN.md:98 reserves the pipeline slot;
  the 2026 survey says ASR+LLM is now the baseline for "premium" feel):
  local CPU inference can't hit a comfortable budget today (~2s+ for a
  paragraph on a strong CPU with a 3B model; research 2026-06-11), so the
  real design question is BYOK-cloud vs local-GPU vs skip. Needs its own
  design plan once delivery + settings exist — a cleanup stage is
  pointless while output lands on stdout.
- **History/database** (PLAN.md:203 keeps it; survey: table stakes):
  searchable transcription history with raw + processed text. Plan after
  settings; design storage around `RawTranscript`/`ProcessedDictation`
  per PLAN.md:210.
- **Elapsed-time/timer in the overlay** (PLAN.md:206): next layer on plan
  005's phase enum.
- **macOS/Windows targets**: GPUI is cross-platform but every delivery and
  hotkey mechanism here is Wayland-specific; keep seams
  (`DeliveryTarget`, overlay handle) platform-clean and port after the
  Linux story is undeniable.
- **Packaging/distribution + systemd unit, structured logging**: already
  tracked in `.agents/plans/gpui-rewrite-hardening/README.md` Deferred.
