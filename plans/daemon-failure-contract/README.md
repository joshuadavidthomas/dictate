# Daemon Failure-Contract Hardening

**Source roadmap:** `.agents/ROADMAP.md` (Now #2, "Daemon failure-contract hardening"; strategic read §"The daemon's failure contract is the biggest correctness debt")
**Source feature artifacts:** N/A (audit-sourced)
**Planned at:** 2026-07-05, working copy `mqpsnkknoowr` / git `0830c547` (parent `8bcd582` "Re-true README and PLAN.md against the current codebase")
**Scope:** `src/daemon.rs`, `src/dictation.rs`, `src/delivery.rs`, `src/mic.rs`, `src/models.rs`
**Planner:** roadmap-to-improve-plans session, 2026-07-05

## Purpose

The daemon is a resident background process, but its failure contract is
"one hiccup, restart the daemon." The mic worker is a single-shot closure:
any error — mic grabbed by another app, USB unplug, a dropped connection
during first model download — exits the worker permanently and lands in
`Unavailable`, a state with no exit transition. `deliver()` carries a
vestigial `Result` the daemon treats as fatal, while its real failure mode
(`println!` EPIPE panic) bypasses error handling entirely and wedges the
state machine in `Transcribing` with the overlay stuck visible. Truncated
model downloads report success. Ring-buffer overflow drops audio silently.
A persistent socket accept error hot-spins the command loop.

Every future daemon feature (insert delivery, live partials, systemd unit)
stacks on this contract. This effort fixes it once, before more gets built
on top.

## What Better Means

- A transient failure (mic open, stream death, delivery, client I/O) returns
  the daemon to `Idle` with a one-time stderr report; the next
  `dictate record start` works. Only fatal init failures (model download,
  extraction, recognizer creation) reach `Unavailable`, and
  `UNAVAILABLE_MESSAGE`'s "restart `dictate daemon`" advice is then always
  true.
- No input the outside world controls (broken stdout pipe, truncated HTTP
  body, absent mic, stalled client) can panic a worker thread or wedge a
  phase.
- Failures the user can't act on are still visible: dropped samples are
  counted and reported; truncated downloads are errors, not corrupt
  successes.
- Regression bar: the existing 60+ unit tests (dictation state machine,
  socket, mic worker, formatter) keep passing untouched, and
  `mic_session_action`'s open/close semantics are unchanged.

## Current State

All six defects verified against working copy `mqpsnkknoowr` (2026-07-05):

- `src/daemon.rs:139-199` — worker closure: `?` on `ensure_downloaded`
  (:141), `create_recognizer` (:142), `mic::capture` (:154), and
  `delivery::deliver` (:183) all funnel to the terminal catch at :196-200
  (`mark_unavailable`). `src/dictation.rs` has no transition out of
  `Unavailable` (start/stop/cancel all return `Busy`, :147/:171/:189).
- `src/delivery.rs:18` — `deliver` returns `Result`, but every arm returns
  `Ok` (clipboard failure falls back to stdout at :38). **Drift from the
  roadmap row:** the clipboard fallback already landed, so the `Err` is now
  unreachable in practice — the dishonest signature and the daemon-side
  fatal `?` remain.
- `src/delivery.rs:46` — `deliver_stdout` uses `println!`, which panics on
  EPIPE. A panic unwinds past the worker's `if let Err` catch: no
  `mark_unavailable`, no `overlay.hide()` (:191), no
  `finish_transcription()` (:192) — phase wedged in `Transcribing`, overlay
  stuck on screen.
- `src/models.rs:141-183` — `download_file` reads to EOF; `content_length`
  (:155) is used only for progress reporting. A truncated body returns
  `Ok(())`.
- `src/mic.rs:213` — `let _ = producer.push(sample);` silently drops audio
  when the ring is full; `src/mic.rs:185-189` — a cpal stream error calls
  `mark_unavailable`, bricking the session on a transient device error.
- `src/daemon.rs:93-99` — socket accept error → `eprintln!` + `continue`; a
  persistent error (e.g. EMFILE) spins the loop hot.

## Desired End State

The daemon has an explicit, tested failure contract: the classification
table below is enforced in code, `Unavailable` is reachable only from fatal
init, the worker loop is panic-free and infallible after init, downloads
verify their length, dropped samples are counted, and the accept loop backs
off under persistent errors.

## Failure Classification (the contract — review gate for this effort)

The roadmap's autonomy note: *"Routine execution once the fatal/retryable
classification is reviewed."* This table is that classification. Approving
this README approves it; plan 002 must not start before then.

| Failure | Class | Resulting state | Surface |
|---|---|---|---|
| Model download / extraction / recognizer init fails | **Fatal** | `Unavailable` (restart required) | one-time `transcription failed: …` + `UNAVAILABLE_MESSAGE` on later commands |
| Mic open fails (`mic::capture` Err) | Retryable | `Recording` → `Idle`, no auto-retry | one-time stderr report; user re-issues `record start` |
| cpal stream error mid-recording | Retryable | `Recording` → `Idle` (captured audio discarded), overlay hidden | stderr report |
| Delivery fails (stdout write error / clipboard error) | Retryable | state untouched; utterance flow completes normally | stderr report (clipboard already falls back to stdout) |
| Truncated model download | Fatal (init-time), but **honest**: an `Err`, never a corrupt success | `Unavailable` | error names bytes received vs expected |
| Ring-buffer overflow | Lossy-continue | recording continues | dropped-sample count reported |
| Socket client read timeout / bad JSON | Retryable (already handled) | loop continues | existing stderr messages |
| Socket `accept()` error | Retryable | loop continues **with backoff** | stderr report |

Rationale (from `coding-standards` `error-handling.md`): fatal init failures
are the only ones where "restart the daemon" is genuinely the caller's next
action; everything else has a cheaper recovery the contract must expose.
"No auto-retry on mic open" is the roadmap's spin guard: aborting
`Recording` → `Idle` means `mic_session_action` never re-opens until the
user acts, so a genuinely absent mic costs one error line per attempt, not
an error loop.

## Source Summary

| Opportunity or slice | Source type | Audit category | Standards concern | Impact | Effort | Risk | Confidence | Source evidence |
|---|---|---|---|---|---|---|---|---|
| Daemon failure-contract hardening | Roadmap (Now #2) | correctness | error-handling / effects (`coding-standards`: failure contracts, expected-failure-vs-defect, honest effect signatures) | Transient errors stop bricking the session; honest `deliver`; no wedged `Transcribing` | M total (1×M + 4×S) | MED (touches the state machine plans 003–005 hardened; good tests exist) | HIGH | `.agents/ROADMAP.md:121`; line-level evidence re-verified above |

## Plan Order

| Plan | Status | Audit category | Standards concern | Depends on | Ready for routine execution? | Needs deeper planning? | Autonomy boundary | Notes |
|---|---|---|---|---|---|---|---|---|
| [001-infallible-delivery](001-infallible-delivery.md) | Not started | correctness | effects / error-handling | None | Yes | No | routine execution | Kills the EPIPE panic + vestigial `Result`; shrinks 002's error surface |
| [002-worker-error-classification](002-worker-error-classification.md) | Not started | correctness | error-handling | 001 | Yes, **after the classification table above is approved** | No | design review already folded into this README's table | The core plan; touches `dictation.rs` + `daemon.rs` + `mic.rs` |
| [003-download-length-verification](003-download-length-verification.md) | Not started | correctness | error-handling (boundary classification) | None | Yes | No | routine execution | Isolated to `src/models.rs`; can run in parallel with 001/002 |
| [004-mic-drop-accounting](004-mic-drop-accounting.md) | Not started | correctness / DX | effects (visible loss) | 002 (file order only) | Yes | No | routine execution | Touches `src/mic.rs`; sequence after 002 to avoid conflicts |
| [005-accept-error-backoff](005-accept-error-backoff.md) | Not started | correctness | error-handling | None (avoid running concurrently with 002 — both edit `src/daemon.rs`) | Yes | No | routine execution | Smallest plan; fine to land first or last |

Status values: TODO | IN PROGRESS | DONE | BLOCKED (one-line reason) |
SUPERSEDED (one-line pointer to what replaced it). Update your row when done.

## Dependency Notes

- **001 → 002**: both edit the worker loop in `src/daemon.rs`
  (:139-199). 001 removes the `deliver(...)?` at :183, which is one of the
  four error sources 002 must classify; landing 001 first means 002 only
  reasons about init errors and mic-open errors.
- **002 → 004**: no logical dependency, but both touch `src/mic.rs`
  (002 rewires `StreamErrorHandler`, 004 instruments the callback at :213).
  Sequential execution avoids re-planning conflicts.
- **003 and 005 are free**: `src/models.rs` and the accept loop are
  untouched by the other plans. Just don't run 005 *concurrently* with
  001/002 (same file).

## Verification Baseline

Established and green at planning time (CI mirrors these in
`.github/workflows/ci.yml`):

- `just check` — compiles, all targets
- `just test` — unit tests incl. the dictation state machine and socket suites
- `just clippy` — `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo +nightly fmt --check` — formatting gate
- `just test-integration` — model-download-dependent corpus tests; **not
  required** for these plans (no ASR behavior changes) but useful as a
  manual smoke for 003 since it exercises `ensure_downloaded`.

## Evals / Regression Checks

- The dictation state-machine tests in `src/dictation.rs` are the effort's
  spine: every plan that adds a transition adds tests there, and none may
  weaken existing ones.
- `mic_session_action_tracks_phase_and_open_state` (`src/daemon.rs`)
  guards the open/close decision table 002 relies on for its no-spin
  argument — it must remain untouched and passing.
- Executor failure mode to watch: "fixing" a retryable path by adding
  auto-retry. The contract is *return to Idle and wait for the user*;
  auto-retry is explicitly deferred (see below).

## Autonomy Boundary

| Action type | Routine execution allowed? | Needs design review? | Needs human approval? |
|---|---|---|---|
| Implementation within a plan's scope, incl. new `DictationControl` transitions specified by the plan | yes | no | no |
| Changing the failure classification table above | no | no | yes (it is the reviewed contract) |
| Adding auto-retry, self-healing, or leaving `Unavailable` without restart | no | yes | yes (deferred feature) |
| Changing user-visible message wording beyond what plans specify | yes (keep the existing stderr style) | no | no |
| New dependencies | no | yes | no |

## Drift Checks Before Any Plan

- Re-read this index and the plan.
- `jj log -r @ --no-graph` and compare against `Planned at`
  (`mqpsnkknoowr` / git `0830c547`); then
  `jj diff --from 0830c547 -- src/daemon.rs src/dictation.rs src/delivery.rs src/mic.rs src/models.rs`
  — if in-scope files changed, re-verify the plan's Current-State Evidence
  before editing; on mismatch, STOP.
- Confirm `just check` and `just test` pass before your first edit.

## Deeper Planning Candidates

None. All five plans are high-confidence with settled shapes. The one
design-sensitive artifact — the failure classification — is resolved in
this README rather than deferred.

| Plan/opportunity | Why it needs depth | Suggested next artifact |
|---|---|---|
| — | — | — |

## Standing Policies / Decisions

| Decision or policy | Why it should not be re-litigated | Where to record or enforce it |
|---|---|---|
| `Unavailable` means "restart required" and is reachable **only** from fatal init failures | The whole contract collapses if transient paths can set it again | Doc comment on `DictationPhase::Unavailable` + `mark_unavailable` (plan 002), enforced by 002's tests |
| `deliver()` is infallible from the caller's view; delivery problems are reported, never propagated as state changes | Prevents re-growing a fatal `?` on an effect that always has a fallback | Signature change in plan 001 (no `Result` to misuse) |
| No automatic retry of retryable failures | Roadmap risk note: retry must not spin on a genuinely absent mic; user-initiated retry is the spin guard | This README + plan 002's Rejected Approaches |

## Considered and Rejected

| Idea | Audit category | Reason rejected | Revisit if |
|---|---|---|---|
| Checksum verification of model downloads | correctness | Already rejected in `plans/gpui-rewrite-hardening/README.md` — sherpa-onnx publishes no stable per-archive checksum manifest; HTTPS from GitHub releases. Length check (plan 003) is the cheap honest middle | a release/packaging story needs reproducibility |
| Transcribe partial audio on mid-recording stream error | correctness / UX | Surprise delivery of a half-utterance with a possibly corrupt tail; contract says abort to Idle. Cheap to add later behind the same `abort_recording` seam | overlay phase states land and can *show* "mic died, kept partial" |
| Bounded auto-retry with backoff for mic open | correctness | Complexity + the roadmap's spin risk; user-initiated retry costs nothing since the state returns to Idle | a hotkey-only, no-terminal deployment makes silent one-line failures invisible |
| Making `DictationControl::apply` return typed errors instead of `DictationUpdate` | architecture | The update enum already is the failure contract at that seam; churn without new information | the command surface grows past four verbs |
| Switching stderr reporting to `log`/`tracing` in this effort | DX | Real, already deferred in the gpui-hardening effort; mixing it in doubles every plan's diff | structured-logging effort starts (pre-systemd) |

## Deferred

| Idea | Why deferred | Trigger to revisit |
|---|---|---|
| Worker auto-retry / self-healing out of `Unavailable` | Deliberately out of contract (see standing policies); also deferred by gpui-hardening plan 003 | daemon gets a supervisor (systemd unit) or health-check surface |
| Overlay error-phase rendering (show `Unavailable`/abort visually) | Roadmap sequences it after this effort ("error states must exist to render") | this effort lands; revise `plans/product-direction/005-overlay-phase-states.md` |
| Structured logging | Separate effort per gpui-hardening deferral | before packaging/systemd work |

## Reconciliation Log

- **2026-07-05** — Effort created from `.agents/ROADMAP.md` Now #2 via
  `roadmap-to-improve-plans`. All six evidence sites re-verified against
  working copy `mqpsnkknoowr`; one drift found and absorbed: the roadmap's
  "unreachable `Err` in `deliver`" (cited `src/delivery.rs:28-43`) is now
  the clipboard→stdout fallback landed by the clipboard-delivery plan, so
  plan 001 removes the vestigial `Result` rather than adding a fallback.
  Failure classification table written for review; 002 gated on it.
