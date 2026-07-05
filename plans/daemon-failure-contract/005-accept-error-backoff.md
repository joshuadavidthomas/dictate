# 005 — Back Off on Socket Accept Errors Instead of Hot-Spinning

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

**Source item:** `.agents/ROADMAP.md` Now #2 "Daemon failure-contract hardening" — defect: "`src/daemon.rs:93-99` accept-error hot spin"
**Effort index:** [README.md](README.md)
**Planned at:** 2026-07-05, working copy `mqpsnkknoowr` / git `0830c547`
**Depends on:** none logically; do not run **concurrently** with 001/002 (same file). Fine to land first or last.
**Executor target:** routine execution ready — yes
**Source type:** roadmap
**Audit category:** correctness
**Standards concern:** `coding-standards` `error-handling.md` — an error loop that retries at maximum speed treats an operational failure as free; the retry policy must be explicit
**Impact:** a persistent accept failure (fd exhaustion, listener invalidated) degrades to a paced, log-readable error stream instead of pegging a core and flooding stderr
**Effort:** S
**Risk:** LOW — a sleep in an error branch of one loop; success path untouched
**Confidence:** HIGH
**Source direction:** roadmap fix sketch: "backoff on accept error"

## Purpose

`run_in_background`'s command loop handles a failed `accept()` with
`eprintln!` + `continue`. `DaemonSocket::accept` internally maps *client*
I/O problems (read timeout, bad JSON, empty payload) to `Ok(None)`, so an
`Err` here means the **listener itself** failed — `accept(2)` errors like
`EMFILE`/`ENFILE` are typically persistent for seconds or forever. The
current shape retries instantly: a hot spin that pegs a core and makes
stderr unusable exactly when the operator needs it.

## What Better Means

- Under a persistent accept error the loop sleeps between attempts, backing
  off to a bounded cap, and recovers to full responsiveness immediately
  after one successful accept.
- Regression bar: zero added latency on the success path; all existing
  socket tests pass unmodified.

## Current-State Evidence

- `src/daemon.rs:91-99` — the loop:

  ```rust
  let command = match self.socket.accept() {
      Ok(Some(command)) => command,
      Ok(None) => continue,
      Err(error) => {
          eprintln!("failed to read record command: {error:#}");
          continue;
      }
  };
  ```

- `src/daemon.rs:266-290` — `DaemonSocket::accept`: only
  `self.listener.accept()?` and `set_read_timeout(...)?` can produce the
  `Err` arm; all per-client read/parse failures return `Ok(None)` (:271-289).
- The `Err` arm's message ("failed to read record command") also
  misattributes a listener failure to a client read — worth fixing while
  here.

## Desired End State

The `Err` arm reports `failed to accept record connection: {error:#}` and
sleeps per an exponential backoff (e.g. 50ms doubling to a 5s cap), reset
by any `Ok` result. The policy lives in a tiny testable unit rather than
inline arithmetic.

## Scope

- `src/daemon.rs` — the `Err` arm in `run_in_background`, a small
  `Backoff` helper + tests

## Out of Scope

- Client-side error handling in `DaemonSocket::accept` (already correct:
  `Ok(None)` + continue).
- Rebinding/recreating the listener on persistent failure (self-healing is
  outside this effort's contract; the daemon still logs and stays up).
- Async/event-loop rearchitecture.

## Design Claim

`coding-standards` `error-handling.md`: operational failures need an
explicit, bounded retry policy — "retry instantly forever" is the absence
of a policy. The helper makes the policy a named, tested value instead of
an accident of `continue`.

## Architecture Diagnosis

N/A (loop hygiene fix).

## Implementation Sequence

### Step 1 — Backoff helper

A minimal private struct in `src/daemon.rs`:

```rust
struct Backoff { current: Duration }
```

with `const BASE`/`MAX` (suggest 50ms / 5s), `fn next(&mut self) -> Duration`
(returns current, then doubles saturating at MAX) and `fn reset(&mut self)`.
No dependency, no jitter — single-consumer local socket, thundering herd
does not apply.

### Step 2 — Wire into the loop

In `run_in_background`: on `Err`, report with the corrected message and
`thread::sleep(backoff.next())`; on either `Ok` arm, `backoff.reset()`.

### Step 3 — Tests

In the existing `src/daemon.rs` tests module:

- `Backoff` sequence: 50ms, 100ms, …, caps at 5s, stays capped.
- `reset` returns it to base.
- Existing socket tests (`slow_client_does_not_block_accept_loop`,
  `ignores_empty_clients`, `reclaims_stale_socket_path`) unmodified —
  they exercise `Ok(None)` paths, which must not sleep.

## Verification

### Automated

- [ ] `just check` — exit 0
- [ ] `just test` — backoff tests + existing socket suite pass
- [ ] `just clippy` — exit 0
- [ ] `cargo +nightly fmt --check` — exit 0

### Evals / Regression Checks

- [ ] `Ok(None)` paths contain no sleep (review the diff): a slow client
      must not throttle the next client's command.
- [ ] The corrected error message distinguishes accept failure from client
      read failure (`rg -n 'failed to accept' src/daemon.rs`).

### Manual

- [ ] None. (Forcing a real `EMFILE` is not worth a harness; the policy is
      unit-tested and the wiring is three lines.)

## Autonomy Boundary

Routine execution may include:

- Everything above, including tuning BASE/MAX within the same order of
  magnitude.

Design review is required for:

- Listener rebinding/self-healing or exiting the process on persistent
  failure.

Human approval is required for:

- Nothing within scope.

## Drift Checks

Before editing, the executor must:

- [ ] Re-read this plan and the effort index.
- [ ] Coordinate with 001/002 status (same file — land sequentially).
- [ ] `jj diff --from 0830c547 -- src/daemon.rs` — re-verify the loop shape
      on any change.
- [ ] Confirm `just test` passes before the first edit.

## STOP Conditions

Stop and hand back if:

- the command loop has been restructured (e.g. by plan 002 drift or an
  async migration) so the `Err` arm no longer matches;
- validation commands fail before changes.

## Rejected Approaches

- **Fixed sleep instead of backoff** — either too slow to recover from a
  one-off blip or too fast under a persistent fault; backoff is four lines
  more.
- **Exit the daemon on persistent accept failure** — turns a degraded-but-
  alive daemon (worker and overlay still function) into a dead one; the
  effort's contract reserves terminal outcomes for fatal init.
- **Recreate the listener** — self-healing machinery out of contract; noted
  as a future supervisor concern in the effort index's deferrals.

## Standing Policy Updates

None.

## Executor Notes

- Keep `Backoff` private to `src/daemon.rs`; do not generalize it into a
  util module for one consumer.
- `POLL_INTERVAL`/`CLIENT_READ_TIMEOUT` constants at the top of the file
  are the precedent for where BASE/MAX belong.
- Update this plan's row in [README.md](README.md) when done.
