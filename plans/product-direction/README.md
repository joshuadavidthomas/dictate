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

**Cross-effort dependency**: `plans/gpui-rewrite-hardening/` touches the
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
| [003](003-settings-foundation.md) | TOML settings unlock the formatter and model catalog | M | 001 | TODO |
| [004](004-default-model-parakeet.md) | Evaluate Parakeet default; retire the 30s ceiling | S–M | hardening 004 | TODO |
| [005](005-overlay-phase-states.md) | Overlay recording/transcribing/error states | M | hardening 003, 005, 006 | TODO |
| [006](006-live-partials-spike.md) | Spike: live partials without leaving sherpa-onnx | S–M | 004 | TODO |

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
  parallel-safe. Its positive verdict spawns two follow-up plans (overlay
  text surface, daemon partials pipeline) — see its maintenance notes.

## Reconciliation log

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
  unproven CPU performance — rejected while partials are overlay
  cosmetics rather than delivered text. The cheap in-sherpa alternatives
  (periodic offline re-decode, two-pass hybrid) are **not** rejected:
  plan 006 spikes them. Re-open the runtime question only if 006's
  verdict is negative AND streaming becomes the delivered text (the
  continuous/meeting path), or when sherpa-onnx ships streaming Moonshine.
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
  tracked in `plans/gpui-rewrite-hardening/README.md` Deferred.
