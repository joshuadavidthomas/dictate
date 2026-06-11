# Plan 003: Harden the daemon against hangs, zombie states, and the overlay race

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from dd6db2c175a3 -- src/daemon.rs src/mic.rs src/models.rs src/dictation.rs`
> If in-scope files have changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: LOW–MED (small targeted fixes on the daemon's threads; each is
  independently verifiable)
- **Depends on**: 001 (CI gate). Coordinate with 004/005, which touch the
  same files — execute 003 → 004 → 005 in order.
- **Planned at**: revision `mtnsrkmyruyz` (git `dd6db2c175a3`), 2026-06-11

## Why this matters

The daemon is a long-lived background process controlled over a Unix socket,
and several failure paths currently hang or wedge it permanently with no
recovery and poor feedback:

1. A client that connects and never closes blocks the command loop forever.
2. A stalled model download hangs the microphone worker forever on startup.
3. If startup fails (no network, no mic), the daemon becomes a permanent
   zombie whose only symptom is the cryptic message "cannot change recording
   while Transcription unavailable".
4. A microphone stream error (device unplugged) is printed once and otherwise
   ignored; subsequent recordings silently capture nothing.
5. A small race can hide the overlay immediately after a new recording
   starts.

Each fix is small; together they make the daemon trustworthy enough to bind
to a global hotkey and forget about.

## Current state

- `src/daemon.rs:180-192` — `DaemonSocket::accept` does a blocking
  `accept()` then `read_to_string` (reads until client EOF) with **no read
  timeout**. One stalled client (`nc -U $XDG_RUNTIME_DIR/dictate.sock` and
  walk away) wedges the command loop permanently.
- `src/daemon.rs:155-178` — `DaemonSocket::bind` resolves
  `XDG_RUNTIME_DIR` itself; binding is not parameterized by path, so it
  can't be tested without touching the real runtime dir.
- `src/models.rs:137-141` — `download_file` starts with:

  ```rust
  let mut response = ureq::get(url)
      .call()
      .map_err(|error| anyhow!("failed to download {url}: {error}"))?;
  ```

  No agent/config timeouts are set anywhere (`rg -n "timeout" src/` → no
  hits). ureq 3.3 applies no default global timeout, so a TCP connection
  that stalls mid-body hangs the worker thread forever. The files are large
  (Whisper models are hundreds of MB), so a small *global* timeout is wrong —
  a connect timeout plus a stall-detection timeout is needed.
- `src/daemon.rs:103-146` — `spawn_microphone_worker`: on any init error
  (model download, recognizer creation, mic open) it prints
  `transcription failed: ...` once and calls `dictation.mark_unavailable()`;
  the thread exits and nothing ever leaves `Unavailable`.
- `src/daemon.rs:95-97` — the user-facing message for that state:

  ```rust
  DictationUpdate::Busy(phase) => {
      eprintln!("cannot change recording while {}", phase.label());
  }
  ```

  With `DictationPhase::Unavailable.label()` == "Transcription unavailable"
  (`src/dictation.rs:61-70`), the user sees
  "cannot change recording while Transcription unavailable" and no hint that
  a daemon restart is required.
- `src/mic.rs:113-127` — `build_input_stream` passes an error callback of
  `|error| eprintln!("recording error: {error}")`. A dead stream is
  otherwise indistinguishable from silence.
- `src/daemon.rs:124-137` — transcription completion in the worker loop:

  ```rust
  dictation.finish_transcription();
  overlay.hide();
  ```

  Between those two calls, the state is already `Idle`, so a concurrent
  `start` command can transition to `Recording` and call `overlay.show()` —
  then the worker's `overlay.hide()` lands and hides the overlay for the
  new, live recording. Hiding **before** finishing closes the race: a start
  arriving then is rejected as Busy(Transcribing), and show can only happen
  after the hide message is already queued (overlay messages are processed
  in order by `src/app.rs:64-98`).
- Conventions: errors via `anyhow` with `anyhow!` context (exemplar:
  `src/daemon.rs:26-36`); typed state transitions live in `DictationControl`
  (`src/dictation.rs:104-221`); tests are `#[cfg(test)] mod tests` per file
  (exemplar: `src/daemon.rs:201-224`).

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| ureq docs | `cargo doc -p ureq` then read `target/doc/ureq/index.html` | docs built |

## Scope

**In scope** (the only files you should modify):
- `src/daemon.rs`
- `src/models.rs`
- `src/mic.rs`
- `src/dictation.rs` (only if the Unavailable messaging needs a new
  `DictationUpdate` arm or label change)

**Out of scope** (do NOT touch):
- `src/text.rs`, `src/transcription.rs` — owned by plan 002 / unaffected.
- Recording-duration caps (`record_samples`) — plan 004.
- Mic open/close lifecycle while idle — plan 005. Do not restructure
  `capture()`'s always-on shape here; only its error callback changes.
- Auto-retry/restart of the worker after failure — deferred (see effort
  README); this plan only makes the failure state visible and the hangs
  impossible.

## Steps

### Step 1: Make `DaemonSocket` testable and add a client read timeout

Refactor so the socket path is injected: e.g. `DaemonSocket::bind_at(path:
PathBuf)` holding today's stale-socket logic, with the `XDG_RUNTIME_DIR`
resolution staying in a small `socket_path()` helper used by both `bind()`
and `send` (`src/daemon.rs:26-36` duplicates the resolution today — unify
it).

In `accept`, set a read timeout on the accepted stream
(`UnixStream::set_read_timeout`) before reading — a short constant (~2s) is
plenty for a same-machine echo of a few bytes. On timeout or read error,
log via the existing `eprintln!` pattern and return `Ok(None)` so the loop
continues serving the next client.

**Verify**: `just test` → all pass, including the new timeout test (Step 5).

### Step 2: Add timeouts to the model download

In `src/models.rs`, build the request from a configured agent/config rather
than bare `ureq::get`. Requirements (consult the ureq 3.3 docs via `cargo
doc -p ureq` for the exact API — `Agent`/config builder timeouts):

- Connect timeout: ~10s.
- A stall must surface as an error within ~60s — prefer per-read/recv-body
  style timeouts over one global timeout, because healthy multi-hundred-MB
  downloads can legitimately take many minutes. If ureq 3.3 only offers a
  whole-body timeout, set it generously (≥15 min) and note that in the PR.
- Error message keeps the existing `failed to download {url}: ...` shape.

**Verify**: `just check` → exit 0; `just test` → all pass. (No network test —
behavior verified by type/docs, not by a live stall.)

### Step 3: Make the Unavailable state say what to do

When a command arrives while the daemon is `Unavailable`, the message must
tell the user the daemon needs a restart, e.g.
"transcription is unavailable (startup failed); restart `dictate daemon`" —
distinct from the transient "cannot change recording while Transcribing…".
Shape is yours: special-case `DictationUpdate::Busy(DictationPhase::Unavailable)`
in the command loop, or add a dedicated update arm. Also include the
underlying startup error in the one-time `transcription failed: {error:#}`
message — it already prints; just confirm it stays the anyhow `{:#}` chain.

**Verify**: `just test` → all pass; `rg -n "restart" src/daemon.rs` → the new
message exists.

### Step 4: Surface mic stream death and close the overlay race

- In `src/mic.rs`, give the stream error callback teeth: pass clones of
  `DictationControl` and `Overlay` into `build_input_stream` so the callback
  reports the error (`eprintln!` stays), calls `dictation.mark_unavailable()`,
  and hides the overlay. (Both types are cheap `Arc`-backed clones; the
  callback runs on an audio thread, and both methods are thread-safe —
  `Overlay` only sends on an unbounded channel, `mark_unavailable` takes a
  mutex.)
- In `src/daemon.rs`, swap the completion order so it reads
  `overlay.hide(); dictation.finish_transcription();` — closing the race
  described in Current state.

**Verify**: `just check` → exit 0; `rg -n "finish_transcription" src/daemon.rs`
→ the call appears after `overlay.hide()` in the worker loop.

### Step 5: Tests

In `src/daemon.rs`'s existing `tests` module:

- **Slow-client timeout**: `bind_at` a socket under `std::env::temp_dir()`
  with a process-unique name (keep the path short — Unix socket paths cap at
  ~108 bytes). Connect a `UnixStream`, write a partial payload, do **not**
  close it; assert `accept()` returns (`Ok(None)` or an error you map to a
  continue) within the timeout rather than blocking. Use a sub-second test
  timeout if you parameterize it; otherwise the 2s constant is acceptable
  test latency.
- **Stale socket reclaim** (locks in existing `bind` behavior now that it's
  testable): bind, drop, bind again at the same path → second bind succeeds.
- Keep the two existing wire-format tests passing unchanged.

**Verify**: `just test` → all pass; `cargo clippy --all-targets -- -D warnings`
→ exit 0.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `just test` → all pass, including ≥2 new daemon socket tests
- [ ] `cargo clippy --all-targets -- -D warnings` → exit 0
- [ ] `rg -n "set_read_timeout" src/daemon.rs` → present
- [ ] `rg -n "timeout" src/models.rs` → present
- [ ] Only in-scope files modified (`jj st`)

## STOP conditions

Stop if:

- The code at the "Current state" locations doesn't match the excerpts.
- ureq 3.3 offers no timeout mechanism that distinguishes a stalled download
  from a slow-but-healthy large download — handback with the exact API
  surface you found.
- cpal's error-callback constraints (Send/lifetime) make passing
  `DictationControl`/`Overlay` clones into it infeasible — handback rather
  than introducing globals.
- Fixing the overlay race appears to require restructuring the worker loop
  beyond reordering the two calls.

On stopping, write a **handback**: current state, desired outcome, lingering
questions. Descriptive, not prescriptive.

## Maintenance notes

- Plan 005 will restructure mic lifecycle (open per recording session); the
  error-callback wiring added here must survive that refactor — it becomes
  per-session wiring.
- Worker auto-retry (leaving `Unavailable` without a daemon restart) was
  deliberately deferred; if added later, `mark_unavailable` likely grows a
  reason payload.
- Reviewers should scrutinize the read-timeout path for partial JSON: a
  client that sends half a command then stalls must not crash the loop or
  leave the listener in a bad state.
