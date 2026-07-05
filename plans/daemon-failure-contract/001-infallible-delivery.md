# 001 — Make `deliver` Infallible and Panic-Proof

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

**Source item:** `.agents/ROADMAP.md` Now #2 "Daemon failure-contract hardening" — the "honest `deliver` contract" and "`println!` EPIPE panic skips state reset" defects
**Effort index:** [README.md](README.md)
**Planned at:** 2026-07-05, working copy `mqpsnkknoowr` / git `0830c547`
**Depends on:** none
**Executor target:** routine execution ready — yes
**Source type:** roadmap
**Audit category:** correctness
**Standards concern:** `coding-standards` `effects.md` (effect contracts: a delivery effect whose signature lies about failure) and `error-handling.md` (a `Result` no caller can meaningfully branch on is not a contract)
**Impact:** a closed/broken stdout pipe can no longer panic the mic worker mid-utterance, which today wedges the phase in `Transcribing` and leaves the overlay stuck on screen; the daemon stops treating delivery as fatal
**Effort:** S
**Risk:** LOW — one module signature plus one call site; behavior change is only in the broken-pipe path
**Confidence:** HIGH — all paths verified in current code
**Source direction:** roadmap fix sketch: "make `deliver` infallible or per-utterance". Chosen: infallible. The clipboard→stdout fallback that already landed did half the work; this plan finishes the signature and removes the panic.

## Purpose

`deliver` is the last error source inside the worker's utterance loop whose
failure story is dishonest in both directions: the `Result` it returns can
never be `Err` (so the daemon's fatal `?` guards nothing), while its real
failure mode — `println!` panicking on EPIPE — bypasses `Result` entirely
and kills the worker thread with no state cleanup. Fixing this first also
shrinks plan 002's surface: after this plan, the worker loop body has no
`?` left except mic open.

## What Better Means

- Killing the reader of the daemon's stdout (e.g. `dictate daemon | head -1`)
  and then dictating does not panic any thread, does not wedge
  `Transcribing`, and does not strand the overlay; the failure is a stderr
  line and the state machine proceeds to `Idle` as if delivery succeeded.
- `deliver`'s signature no longer advertises a failure that cannot happen.
- Regression: stdout delivery still ends with a newline (scripts consume
  line-oriented output); clipboard fallback messages unchanged.

## Current-State Evidence

- `src/delivery.rs:18-26` — `pub fn deliver(...) -> Result<()>`; the stdout
  arm returns `Ok(())` unconditionally.
- `src/delivery.rs:28-43` — `deliver_to_clipboard` returns `Ok(())` on both
  arms (failure falls back to stdout at :38). No `Err` escapes `deliver`.
- `src/delivery.rs:45-47` — `fn deliver_stdout` uses `println!`, which
  panics if stdout is a broken pipe.
- `src/daemon.rs:183` — `delivery::deliver(delivery, text.as_str())?;`
  inside the worker loop; the `?` routes to the terminal catch at
  `src/daemon.rs:196-200` (`mark_unavailable`).
- `src/daemon.rs:191-192` — `overlay.hide()` and
  `dictation.finish_transcription()` run *after* delivery; a delivery panic
  skips both.

## Desired End State

`pub fn deliver(target: DeliveryTarget, text: &str)` returns nothing.
Stdout writes go through a fallible writer seam that reports failure to
stderr (best-effort) instead of panicking. `src/daemon.rs:183` is a plain
call with no `?`. Worker state cleanup (`overlay.hide()`,
`finish_transcription()`) is unconditionally reached after transcription.

## Scope

- `src/delivery.rs` — signature, stdout write path, tests
- `src/daemon.rs` — the single call site at :183

## Out of Scope

- Worker init/mic-open error classification — plan 002.
- Any change to clipboard behavior or its fallback messages.
- Delivery outcome reporting to the caller (an `InsertionOutcome`-style
  enum is sketched in `plans/product-direction/spike-insertion-findings.md`
  for the future insert target — do not build it here).
- Logging framework changes.

## Design Claim

`coding-standards` `effects.md`: make the effect's consequence visible and
honest — `deliver` always completes from the caller's perspective because
every failure has an internal fallback or report; therefore the type says
so. `error-handling.md`: removing a `Result` that no caller can branch on
is the correction, not a loss of safety — the panic path was never in the
contract.

## Architecture Diagnosis

N/A (single-seam correctness fix).

## Implementation Sequence

### Step 1 — Panic-proof the stdout write

In `src/delivery.rs`, replace `deliver_stdout`'s `println!` with a write to
a `impl std::io::Write` seam so it is testable, e.g.:

```rust
fn write_text(mut out: impl Write, text: &str) -> std::io::Result<()> {
    writeln!(out, "{text}")
}
```

`deliver_stdout` calls it with `std::io::stdout().lock()`; on `Err`, report
via a best-effort stderr write (use `let _ = writeln!(std::io::stderr(), ...)`
rather than `eprintln!`, which also panics on a broken stderr). Message
shape: `failed to write dictation to stdout: {error}` — matches the
existing stderr style.

### Step 2 — Drop the `Result` from `deliver`

Change `deliver` and `deliver_to_clipboard` to return `()` (the clipboard
arm's `Ok(())`s disappear; `copy_to_clipboard` keeps its internal `Result`
— it is a real fallible boundary that the fallback consumes). Update
`src/daemon.rs:183` to a plain call. `cargo check` will find any other
caller (there are none at planning time; `delivery::deliver` appears only
in `src/daemon.rs`).

### Step 3 — Tests

In `src/delivery.rs`'s existing `tests` module:

- `write_text` appends a newline (write to a `Vec<u8>`, assert bytes).
- `write_text` surfaces the error from a failing writer (a tiny
  `struct FailingWriter` whose `write` returns
  `ErrorKind::BrokenPipe`) — proves the EPIPE path is an `Err`, not a panic.
- Keep the two existing `DeliveryTarget` tests unchanged.

## Verification

### Automated

- [ ] `just check` — no remaining caller expects `Result` from `deliver`
- [ ] `just test` — new writer tests pass; existing 60+ tests untouched
- [ ] `just clippy` — exit 0 (will also catch a leftover `unused_must_use`)
- [ ] `cargo +nightly fmt --check` — exit 0

### Evals / Regression Checks

- [ ] `rg -n 'println!' src/delivery.rs` → no hits (the panic vector is gone)
- [ ] `rg -n 'deliver\(' src/daemon.rs` → call has no `?`

### Manual

- [ ] Optional smoke: `dictate daemon | head -0` in one terminal, dictate a
      short utterance via `dictate record toggle`; daemon stays alive,
      stderr shows the write failure, a second dictation still works.
      (Requires a mic; skip if headless — the FailingWriter test covers the
      logic.)

## Autonomy Boundary

Routine execution may include:

- The signature change, writer seam, tests, and call-site update as
  specified.

Design review is required for:

- Any urge to return a delivery outcome/status to the worker — that is the
  insert-delivery effort's design space, not this plan's.

Human approval is required for:

- Nothing within scope.

## Drift Checks

Before editing, the executor must:

- [ ] Re-read this plan and the effort index.
- [ ] `jj diff --from 0830c547 -- src/delivery.rs src/daemon.rs` — on
      changes, re-verify Current-State Evidence.
- [ ] Confirm `just test` passes before the first edit.

## STOP Conditions

Stop and hand back if:

- `deliver` has grown additional callers or an additional target variant
  since planning;
- plan 002 has already landed and restructured the worker loop such that
  :183 no longer matches — reconcile with the index before editing;
- validation commands are missing or fail before changes;
- the change grows beyond one independently reviewable PR.

## Rejected Approaches

- **Keep `Result` and handle `Err` per-utterance in the worker** — keeps a
  dishonest signature and forces every future caller to invent handling for
  an error that cannot occur.
- **Catch the panic with `catch_unwind` in the worker** — treats a defect
  as control flow; the standards line is that expected failures (broken
  pipe on a long-lived daemon's stdout is expected) belong in the contract,
  not in panic recovery.
- **`libc::signal(SIGPIPE, SIG_DFL)`-style suppression** — process-global,
  affects unrelated writes, and hides the loss instead of reporting it.

## Standing Policy Updates

Recorded in the effort index: `deliver()` is infallible from the caller's
view; delivery problems are reported, never propagated as state changes.

## Executor Notes

- `src/delivery.rs` already imports nothing from `std::io`; add what you
  need with one import per line (repo style — see any `use` block).
- Repo style forbids new comments unless the WHY is non-obvious; the
  FailingWriter test needs no commentary.
- Update this plan's row in [README.md](README.md) when done.
