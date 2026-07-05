# 002 — Classify Worker Failures: Fatal → Unavailable, Retryable → Idle

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

**Source item:** `.agents/ROADMAP.md` Now #2 "Daemon failure-contract hardening" — core defect: "worker `?` → terminal `mark_unavailable`, no exit transition"
**Effort index:** [README.md](README.md)
**Planned at:** 2026-07-05, working copy `mqpsnkknoowr` / git `0830c547`
**Depends on:** 001 (removes the `deliver(...)?` from the loop this plan restructures)
**Executor target:** routine execution ready — yes, gated on the failure classification table in [README.md](README.md) being approved
**Source type:** roadmap
**Audit category:** correctness
**Standards concern:** `coding-standards` `error-handling.md` — expected failure vs defect: a transient mic error is an expected failure the contract must let the user recover from; only genuine "restart required" conditions may be terminal
**Impact:** a mic grabbed by another app, a USB unplug, or a stream death no longer bricks the session until daemon restart; `UNAVAILABLE_MESSAGE`'s restart advice becomes always-true
**Effort:** M
**Risk:** MED — touches the dictation state machine that gpui-hardening plans 003–005 reworked; mitigated by that effort's strong test suite and by this plan adding transitions rather than changing existing ones
**Confidence:** HIGH — the defect and fix shape are fully characterized; the classification is settled in the effort index
**Source direction:** roadmap fix sketch: "classify fatal (model init) vs retryable (mic open, stream error, delivery); return to Idle on retryable"

## Purpose

The mic worker (`src/daemon.rs:139-199`) is a single-shot closure: any `?`
anywhere — init or steady-state — exits the thread permanently into
`Unavailable`, a state with no exit transition. For a resident daemon this
is the gap between "hiccup" and "restart the daemon". This plan draws the
line the contract needs: init failures stay terminal; everything after
`mark_ready` returns the user to `Idle`.

## What Better Means

- With the daemon `Idle` and the mic disconnected: `dictate record start`
  produces one stderr report and returns to `Idle`; plugging the mic back
  in and starting again just works. Today the same sequence requires a
  daemon restart.
- A cpal stream error mid-recording aborts to `Idle` (audio discarded,
  overlay hidden, one stderr line) instead of `Unavailable`.
- `Unavailable` is reachable **only** from model download/extraction/
  recognizer-creation failure — the one case where "restart `dictate
  daemon`" (`src/daemon.rs:30`) is true advice.
- Regression bar: every existing test in `src/dictation.rs` and
  `src/daemon.rs` passes unmodified; `mic_session_action`'s decision table
  is untouched.

## Current-State Evidence

- `src/daemon.rs:139-199` — one closure wraps init *and* the infinite loop;
  the catch at :196-200 does `eprintln!` + `overlay.hide()` +
  `mark_unavailable()` for any `Err`.
- `src/daemon.rs:141-142` — `ensure_downloaded()?` /
  `create_recognizer(&model_dir)?` (genuinely fatal: no model, no daemon).
- `src/daemon.rs:154` — `crate::mic::capture(...)?` (transient: device
  busy/absent) exits the worker permanently.
- `src/daemon.rs:183` — `deliver(...)?` — removed by plan 001; after it
  lands, mic open is the **only** `?` left inside the loop.
- `src/mic.rs:185-189` — `StreamErrorHandler::handle` calls
  `dictation.mark_unavailable()` + `overlay.hide()` on any cpal stream
  error.
- `src/dictation.rs:256-258` — `mark_unavailable` overwrites *any* state;
  no method transitions out of `Unavailable` (start/stop/cancel return
  `Busy(Unavailable)` at :147/:171/:189).
- `src/dictation.rs:178-196` — `cancel_recording` already implements
  `Recording → Idle`, but only via the public command surface
  (`apply(Cancel)`), which also emits `DictationUpdate::Cancelled` handled
  by the command loop — the worker needs the transition without the
  command-loop semantics.
- Spin-guard mechanics: `mic_session_action` (`src/daemon.rs:212-224`)
  opens the mic only when `(Recording, false)`. Leaving `Recording` on
  failure therefore stops retry attempts until the user issues a new
  `start` — no timer or retry budget needed.

## Desired End State

Worker init (download, recognizer) keeps today's terminal behavior. The
steady-state loop cannot exit the thread: mic-open failure reports once,
aborts the recording to `Idle`, and continues polling. Stream errors abort
to `Idle` instead of `Unavailable`. A doc comment on `mark_unavailable`
records the contract: only fatal init may call it.

## Scope

- `src/dictation.rs` — one new crate-internal transition (`abort_recording`)
  + tests + contract doc comments
- `src/daemon.rs` — split init from loop in `spawn_microphone_worker`;
  in-loop handling for mic-open failure
- `src/mic.rs` — `StreamErrorHandler::handle` rewiring

## Out of Scope

- Delivery failure handling — plan 001 (must land first).
- Dropped-sample accounting (`src/mic.rs:213`) — plan 004.
- Accept-loop backoff — plan 005.
- Auto-retry of anything (standing policy in the index).
- Overlay visuals for the abort (deferred to the overlay-phase-states plan).
- Any change to `mic_session_action` or existing `DictationControl`
  transitions.

## Design Claim

`coding-standards` `error-handling.md`: "If callers can recover, branch,
retry … the failure is visible in the local contract" and "expected
failures are part of the contract; defects are not." Mic-open failure and
stream death are expected failures of a long-lived audio daemon; treating
them as terminal defects was the contract violation. The classification
table in the effort index is the reviewed failure contract this plan
enforces.

## Architecture Diagnosis

- **Current friction:** one error channel (the closure's `Result`) forces
  every failure, regardless of severity, into the same terminal sink.
- **Deepening direction:** severity lives where the error occurs — the init
  section owns fatal, the loop body owns retryable — so the state machine's
  `Unavailable` doc contract becomes enforceable.
- **Deletion test:** N/A — no module is added or removed; one transition is
  added behind the existing `DictationControl` interface.
- **Locality / leverage claim:** future daemon features (insert delivery,
  partials) run inside this loop; landing the "loop errors are
  session-scoped" rule now means they inherit it instead of re-deciding.
- **Recommendation strength:** Strong.
- **ADR conflicts:** none (no ADRs in repo).

## Implementation Sequence

### Step 1 — Add `abort_recording` to `DictationControl`

In `src/dictation.rs`, add a crate-internal method:

```rust
pub(crate) fn abort_recording(&self) -> bool
```

Transitions `Recording { .. } → Idle` and returns `true`; returns `false`
and leaves state untouched for every other state (including
`Transcribing` — an async stream error arriving after stop must not
destroy a queued utterance, and `Unavailable` must not be resurrected).
Add doc comments to `abort_recording` and `mark_unavailable` stating the
contract: `mark_unavailable` is init-failure-only ("restart required");
`abort_recording` is the retryable path.

Tests (same module, follow the existing `start_test_recording` helper
style): aborts from `Recording` to `Idle`; returns `false` from
`Initializing`, `Idle`, `Transcribing` (queued utterance survives and
`take_utterance` still yields it), and `Unavailable`.

### Step 2 — Split worker init from the loop in `src/daemon.rs`

Restructure `spawn_microphone_worker` so the terminal catch wraps **only**
init. One workable shape (executor may vary the mechanics, not the
semantics):

```rust
thread::spawn(move || {
    let init = || -> Result<OfflineRecognizer> {
        let model_dir = model.ensure_downloaded()?;
        model.create_recognizer(&model_dir)
    }();
    let recognizer = match init {
        Ok(recognizer) => recognizer,
        Err(error) => {
            eprintln!("transcription failed: {error:#}");
            overlay.hide();
            dictation.mark_unavailable();
            return;
        }
    };
    dictation.mark_ready();
    // … loop, now returning nothing …
});
```

The loop body loses its `Result` context entirely (after plan 001 the only
`?` is mic open — handled in Step 3), which makes "the loop cannot kill
the thread" a compile-visible property rather than a convention.

### Step 3 — Handle mic-open failure in-loop

In the `MicSessionAction::Open` arm (`src/daemon.rs:152-168`), replace the
`?` on `crate::mic::capture(...)` with a match: on `Err`, report
`microphone unavailable: {error:#}; returning to idle — run `dictate
record start` to retry` (one stderr line, existing style), call
`dictation.abort_recording()`, and `continue`. Do not call
`overlay.hide()` here unless `abort_recording` returned `true` and the
overlay was shown — at this point in the flow the overlay has not been
shown for this session (show happens only after a successful open, at
:157), so no hide is needed; verify that claim against the live code and
add the hide only if the flow has drifted.

### Step 4 — Rewire `StreamErrorHandler`

In `src/mic.rs:185-189`, replace `mark_unavailable` with
`abort_recording`; hide the overlay only when the abort acted:

```rust
fn handle(&self, error: cpal::StreamError) {
    eprintln!("recording error: {error}");
    if self.dictation.abort_recording() {
        self.overlay.hide();
    }
}
```

Add a unit test in `src/mic.rs`'s tests module if constructible
(`cpal::StreamError::DeviceNotAvailable` is a unit variant): a handler over
a `DictationControl` in `Recording` ends `Idle`; over `Transcribing` (use
the dictation test-style setup or drive via public methods:
start→samples→stop) leaves the phase `Transcribing`. If `Overlay` cannot be
constructed headlessly in a unit test, test the `abort_recording` semantics
in `src/dictation.rs` (Step 1) and leave `handle` untested — note it in
the PR.

### Step 5 — Full-suite verification and message audit

Run the whole gate. Then audit stderr messages:
`rg -n 'unavailable' src/` — every remaining "restart `dictate daemon`"
surface must now be reachable only via fatal init.

## Verification

### Automated

- [ ] `just check` — worker loop compiles without a `Result` context
- [ ] `just test` — new `abort_recording` tests + all existing state-machine
      and socket tests pass unmodified
- [ ] `just clippy` — exit 0
- [ ] `cargo +nightly fmt --check` — exit 0

### Evals / Regression Checks

- [ ] `mic_session_action_tracks_phase_and_open_state` untouched and green —
      guards the no-spin argument.
- [ ] `rg -n 'mark_unavailable' src/` → call sites are exactly: the worker
      init catch (`src/daemon.rs`) and the definition (`src/dictation.rs`).
      Any other hit is a contract violation.
- [ ] Existing tests `recording_stops_to_captured_utterance`,
      `initializing_blocks_recording_until_microphone_is_ready`, and the
      auto-stop suite pass unmodified (no weakening of transitions).

### Manual

- [ ] With a real mic: start the daemon, `dictate record start`, yank the
      USB mic (or `pw-cli` suspend the source) → one stderr line, overlay
      hides, phase returns Idle; replug; `dictate record start` records
      again without restart. (Hardware-dependent — do it if you can; the
      unit tests carry the contract otherwise.)

## Autonomy Boundary

Routine execution may include:

- The transition, worker restructure, and rewiring exactly as classified in
  the effort index's table; mechanical shape variations in Step 2.

Design review is required for:

- Any deviation from the classification table (e.g. deciding some stream
  errors are fatal after all);
- adding any retry/timer/budget mechanism;
- changes to `mic_session_action` (if you think you need one, the plan's
  premise broke — STOP instead).

Human approval is required for:

- Changing the classification table itself (it is the reviewed contract).

## Drift Checks

Before editing, the executor must:

- [ ] Re-read this plan and the effort index (including the classification
      table — it is normative).
- [ ] Confirm plan 001 is DONE in the index and `src/daemon.rs`'s loop has
      no `deliver(...)?`.
- [ ] `jj diff --from 0830c547 -- src/daemon.rs src/dictation.rs src/mic.rs`
      — re-verify Current-State Evidence on any change.
- [ ] Confirm `just test` passes before the first edit.

## STOP Conditions

Stop and hand back if:

- plan 001 has not landed (the loop still contains `deliver(...)?`);
- the classification table in the index has not been approved, or you find
  a failure mode it does not cover;
- `Overlay`/`DictationControl` threading constraints make the Step 4 wiring
  infeasible (both are cheap `Arc`-backed clones today — if that changed,
  the plan is stale);
- closing the mic-open failure path seems to require changing
  `mic_session_action` or existing transitions;
- validation commands fail before changes;
- the change grows beyond one independently reviewable PR.

## Rejected Approaches

- **Auto-retry mic open with backoff** — roadmap's named risk: must not
  spin on a genuinely absent mic. Returning to `Idle` makes the user's next
  `start` the retry, at zero implementation cost. Standing policy in the
  index.
- **Transcribe partial audio on stream error** — surprise delivery of a
  half-utterance with a possibly corrupt tail; recorded in the index's
  Considered-and-Rejected with its revisit trigger.
- **Reuse `apply(DictationCommand::Cancel)` from the worker/callback** —
  entangles internal failure handling with the user-command surface and its
  `DictationUpdate` reporting; a dedicated transition keeps the contract
  legible (and `Cancel` from `Transcribing` returns `Busy`, which is not
  the wanted no-op-with-signal shape).
- **A `WorkerError { Fatal, Retryable }` error enum threaded through one
  `Result` channel** — heavier than needed once init and loop are simply
  separate scopes; severity-by-location is the simpler honest shape here.

## Standing Policy Updates

Recorded in the effort index: `Unavailable` is restart-required and
init-only; no automatic retry of retryable failures. This plan encodes both
in doc comments + the `rg mark_unavailable` regression check.

## Executor Notes

- The nested `if dictation.phase() == DictationPhase::Recording` block at
  `src/daemon.rs:155-165` looks odd but is deliberate double-checking
  around a race (stop/cancel arriving during mic open). Do not "clean it
  up" in this plan.
- `src/dictation.rs` tests use `start_test_recording` and direct state
  injection — follow that style for the `Transcribing`-survives-abort test.
- gpui-hardening plan 003's maintenance note says the stream-error wiring
  "must survive" lifecycle refactors — you are intentionally *changing its
  target state* (Idle, not Unavailable), which supersedes that note; say so
  in the PR description.
- Update this plan's row in [README.md](README.md) when done.
