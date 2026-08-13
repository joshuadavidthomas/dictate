# 004 — Count and Report Dropped Microphone Samples

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

**Source item:** `.agents/ROADMAP.md` Now #2 "Daemon failure-contract hardening" — defect: "`src/mic.rs:213` silent ring-buffer drop"
**Effort index:** [README.md](README.md)
**Planned at:** 2026-07-05, working copy `mqpsnkknoowr` / git `0830c547`
**Depends on:** 002 (file-order only: both touch `src/mic.rs`; no logical dependency)
**Executor target:** routine execution ready — yes
**Source type:** roadmap
**Audit category:** correctness / DX
**Standards concern:** `coding-standards` `effects.md` — hidden effects are bugs waiting for a caller: silently discarding caller-audible audio is an invisible consequence that must at least be observable
**Impact:** when transcripts come back with missing words because the worker stalled and the ring overflowed, stderr says so with numbers, instead of the user blaming the model
**Effort:** S
**Risk:** LOW — one atomic counter; the audio callback path gains a single relaxed `fetch_add` on the overflow branch only
**Confidence:** HIGH
**Source direction:** roadmap fix sketch: "count dropped samples"

## Purpose

The cpal callback pushes into a 192k-sample ring (`AUDIO_RING_SAMPLES`,
`src/mic.rs:32`) and discards on overflow with `let _ =`. At 48kHz input
that ring is ~4s deep — a worker stall (CPU contention, scheduler hiccup
under model load) silently eats audio from the middle of an utterance.
Under this effort's contract, ring overflow is classified lossy-continue:
recording proceeds, but the loss must be visible.

## What Better Means

- Any session that drops samples produces a stderr report with the count
  (and its approximate duration at the input rate) — at first occurrence
  and as a session total.
- Regression bar: zero added work on the non-overflow hot path beyond what
  exists today; no locks, allocation, or I/O in the audio callback
  (realtime constraint); all existing mic/resampler tests pass unmodified.

## Current-State Evidence

- `src/mic.rs:207-217` — `build_input_stream`'s data callback:
  `let _ = producer.push(sample);` (:213) — `rtrb::Producer::push` returns
  `Err(PushError::Full)` on overflow; discarded silently.
- `src/mic.rs:84` — `capture_with_config` creates the ring and already
  wires per-session state (`StreamErrorHandler`) into the callback — the
  counter follows the same pattern.
- `src/mic.rs:221-268` — `audio_worker` loop drains the consumer; it knows
  `input_sample_rate` and exits when the producer is abandoned (:242-244) —
  the natural place to report first-occurrence and session totals without
  touching the callback's realtime budget.
- `src/mic.rs:42-49` — `Mic::drop` joins the worker, so a session-end
  report from the worker thread always lands before the session object is
  gone.

## Desired End State

An `Arc<AtomicU64>` drop counter shared by the callback (increment-only,
`Ordering::Relaxed`) and `audio_worker` (read-only). The worker warns on
first observed drop mid-session and prints a total at session end when
nonzero: `mic ring buffer overflowed; dropped {n} samples (~{ms}ms of audio)`.

## Scope

- `src/mic.rs` only

## Out of Scope

- Resizing the ring, adaptive backpressure, or prioritizing the worker
  thread — tuning was explicitly burned before (gpui-hardening README:
  "audio-worker micro-tuning… tried with no measured effect"); this plan
  observes, it does not tune.
- Feeding drop counts into the state machine or overlay.
- Plan 002's `StreamErrorHandler` changes.

## Design Claim

`coding-standards` `effects.md`: work a module discards is an effect the
caller must be able to observe. The counter makes the loss part of the
session's observable behavior without changing the realtime contract of
the callback.

## Architecture Diagnosis

N/A (observability fix).

## Implementation Sequence

### Step 1 — Thread the counter through

In `capture_with_config`, create `Arc<AtomicU64>`; clone into
`build_input_stream` (add a parameter) and into the `audio_worker` spawn.
In the callback, replace `let _ = producer.push(sample);` with an `if
push failed → dropped.fetch_add(1, Ordering::Relaxed)`. Nothing else in
the callback.

### Step 2 — Report from the worker

In `audio_worker`: track the last-seen counter value; on the first
transition 0 → nonzero, `eprintln!` a warning; on loop exit (producer
abandoned), if the final count is nonzero, `eprintln!` the session total
with approximate milliseconds (`count * 1000 / input_sample_rate as u64`).
`audio_worker` already takes `input_sample_rate`; extend its signature
with the counter (the test at `src/mic.rs:330-336` calls it directly —
update it).

### Step 3 — Tests

- Extend `audio_worker_exits_when_producer_is_dropped` for the new
  parameter (counter at zero → no report path panics).
- Add a test driving `audio_worker` with a pre-loaded counter and a
  dropped producer to cover the session-total branch (assert no panic;
  stderr content is not asserted — repo tests don't capture stderr).
- If you extracted a `fn dropped_duration_ms(samples: u64, rate: u32) -> u64`
  helper, unit-test the arithmetic (48k rate, 48 samples → 1ms).

## Verification

### Automated

- [ ] `just check` — exit 0
- [ ] `just test` — mic tests (worker exit, resampler suite) pass; new
      tests green
- [ ] `just clippy` — exit 0
- [ ] `cargo +nightly fmt --check` — exit 0

### Evals / Regression Checks

- [ ] `rg -n 'let _ = producer.push' src/mic.rs` → no hits (the silent
      discard is gone).
- [ ] Callback body contains no new locks/allocations/`eprintln!` — review
      the diff for realtime safety; the only addition is the relaxed
      `fetch_add` on the overflow branch.

### Manual

- [ ] None required. (Forcing a real overflow needs an artificial worker
      stall; not worth a harness here — the counter logic is fully covered
      by unit tests.)

## Autonomy Boundary

Routine execution may include:

- Everything in the implementation sequence, including the
  `audio_worker` signature change and test updates.

Design review is required for:

- Any reaction beyond reporting (resizing the ring, aborting the session,
  state-machine involvement).

Human approval is required for:

- Nothing within scope.

## Drift Checks

Before editing, the executor must:

- [ ] Re-read this plan and the effort index.
- [ ] Confirm plan 002 is DONE (or explicitly coordinate — same file).
- [ ] `jj diff --from 0830c547 -- src/mic.rs` — re-verify evidence on any
      change (002 will have touched `StreamErrorHandler`; that does not
      affect this plan's sites).
- [ ] Confirm `just test` passes before the first edit.

## STOP Conditions

Stop and hand back if:

- the callback/worker plumbing has been restructured (e.g. overlay spectrum
  moved out of `audio_worker`) so the counter has no natural home;
- adding the counter parameter cascades into more than the three touch
  points named above;
- validation commands fail before changes.

## Rejected Approaches

- **Report from the callback** — `eprintln!` in a realtime audio callback
  can block on stderr; violates the callback's budget.
- **Grow the ring instead** — hides the symptom, unbounded memory under a
  genuinely stalled worker, and gpui-hardening already established that
  blind tuning of this path is wasted effort.
- **Overflow → abort the recording** — over-reaction for what is usually a
  few dropped milliseconds; the contract classifies this lossy-continue.

## Standing Policy Updates

None.

## Executor Notes

- `SpectrumAnalyzer` feeding and `RecordSamplesUpdate` handling in
  `audio_worker` are plan-006/004 (gpui effort) territory — leave them be.
- Keep the report message in the existing lowercase stderr style.
- Update this plan's row in [README.md](README.md) when done.
