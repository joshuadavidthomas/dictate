# Plan 006: Fix the overlay's choppy, low-FPS spectrum animation

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from dd6db2c175a3 -- src/overlay.rs src/mic.rs src/app.rs src/spectrum.rs src/components/waveform.rs Cargo.toml Justfile`
> If in-scope files have changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Effort**: M (small diffs, but verification means instrumented live runs
  on a Wayland session with a microphone, including a control build of the
  pre-regression commit, and a human judging smoothness)
- **Risk**: MED (touches the render loop and the audio capture path;
  every change is gated by measurement against a known-smooth control)
- **Depends on**: none. **File conflict**: plans 003/004/005 also modify
  `src/mic.rs`/`src/daemon.rs` — do not execute concurrently with them.
  This plan addresses the maintainer's most painful issue; running it
  first is recommended.
- **Planned at**: revision `mtnsrkmyruyz` (git `dd6db2c175a3`), 2026-06-11

## The regression, precisely

This is not a "GPUI is slow" mystery — it is a regression with a known
point and a runnable before-state:

- **Before (smooth)**: commit `0719b7d8` ("Simplify daemon and dictation
  domain model", 2026-06-05 14:57). Two processes: the daemon ran mic +
  FFT and spawned `dictate app` (via `current_exe`) as a dedicated GPUI
  overlay process, streaming spectrum frames as text lines over a stdin
  pipe. The maintainer described this as buttery smooth.
- **The change (uncommitted!)**: that evening (~22:26, per the
  2026-06-04/05 pi session log) the overlay child process was folded into
  the daemon process — and, in the same rewrite, the FFT moved out of the
  cpal callback into a new ring-buffer worker draining
  `WORKER_BATCH_SAMPLES = 4096` per iteration. 4096 samples ÷ 16kHz =
  **256ms per batch ≈ 4 content updates/sec**. First complaint six minutes
  later: "HOLY COW … like 5 fps". The maintainer recalls ~2fps.
- **The climb back**: thirteen fixes that night (batch 4096→256, FFT hop
  128, easing, allocation purge, channel rewrites) got it to a perceived
  ~15–25fps. The final rewrite shipped unverified, and it still feels far
  below the pre-regression state.

Everything — regression and all fixes — is in the **uncommitted working
copy** on top of `0719b7d8`. Consider committing the working copy before
executing this plan so its state is pinned.

No measurement was ever taken during any of this. This plan measures
first, uses `0719b7d8` as the control, and fixes what the numbers indict.

## Three investigation areas

Comparing the smooth design (`jj file show -r 0719b7d8 src/...`) against
the working copy isolates what actually changed:

| Area | Smooth (`0719b7d8`) | Current working copy | Verdict |
|------|---------------------|----------------------|---------|
| **A. Data production** | FFT inside the cpal callback; one frame per full 512-sample window (`sample_buffer.clear()`), **even ~31 frames/sec**; analyzer EMA `SMOOTHING_FACTOR = 0.7` (old `src/spectrum.rs:7,51-81`) | ring buffer → worker thread → 256-sample batches → 128-sample hop = nominal 125 frames/sec but **bursty** (burst shape set by PipeWire quantum); **no smoothing in the analyzer**; fast view-side easing instead | changed — prime suspect |
| **B. Transport / consumption** | per-frame mpsc → pipe write → child reader thread → `Arc<Mutex>` set; child renders via 16ms timer + `cx.notify()` sampling latest (old `src/app.rs:188-204`, `src/overlay.rs:12-26`) | atomic latest-value store; same 16ms timer + notify sampling latest (`src/overlay.rs:14-49`) | equivalent at the consumer; the original 2fps contributor (spectrum routed through a coalescing 16–50ms-polled GPUI channel) was already removed mid-session |
| **C. Render pacing / process cohabitation** | overlay GPUI ran in a **dedicated process** containing one stdin-reader thread and nothing else | same GPUI loop now shares a process with the cpal realtime thread, a 1ms-sleep audio worker, the transcription poll thread, the socket accept thread, and sherpa-onnx/ONNX-runtime threads | render mechanism identical in code; cohabitation unproven either way — the controls below discriminate |

**Key inference**: the GPUI render loop (16ms timer + notify + dirty draw)
is the same in the smooth and choppy versions. Suspicion belongs first on
**A** (content cadence and smoothing), with **C** verified cheaply by
control runs — not on the notify loop itself.

## Current state (working copy)

- `src/mic.rs:113-127` — cpal stream built from `config.into()`, leaving
  `buffer_size: BufferSize::Default`: PipeWire's quantum decides callback
  burst size/cadence (commonly 21–43ms). Callback pushes into a
  192,000-sample rtrb ring.
- `src/mic.rs:130-163` — `audio_worker` drains `WORKER_BATCH_SAMPLES = 256`
  batches (1ms sleep when empty), resamples, feeds dictation + FFT.
- `src/spectrum.rs:13-14,82-93` — `FFT_SIZE = 512`, `FFT_HOP_SIZE = 128`
  (8ms of audio per frame), **no EMA**; frames stored latest-writer-wins
  into 8 relaxed `AtomicU32`s (`src/spectrum.rs:27-54`).
- `src/overlay.rs:14-68` — 16ms timer loop: `advance_waveform(); notify()`.
  Exponential easing toward the latest snapshot with `RISE_SPEED = 90.0`
  (~76% of a move completed in one 16ms frame — fast enough to read as a
  step), `FALL_SPEED = 50.0`, `MAX_FRAME_TIME = 0.05`.
- `src/app.rs:103-131` — layer-shell overlay window: `focus: false`,
  `KeyboardInteractivity::None`, 72×40px, transparent.

**Confirmed GPUI constraint** (pinned checkout
`~/.cargo/git/checkouts/zed-a70e2ad075855582/50d001f/`):

- `crates/gpui/src/window.rs:1436-1449` — frames driven by pending
  `next_frame_callbacks` on a window where `!active.get()` are capped at
  ~30fps ("Inactive window (not focused): cap to ~30fps to save energy").
  `request_animation_frame()` is `on_next_frame(notify)`
  (`window.rs:2142-2146`), i.e. exactly that path; `with_animation` is
  built on it. A Wayland window only becomes active on
  `wl_keyboard::Event::Enter` (`wayland/client.rs:1477-1483`) — with
  `KeyboardInteractivity::None` this overlay is **never active**. This cap
  is why the session's framework-native attempts plateaued choppy.
- The plain dirty path is NOT throttled (`window.rs:1436-1441,1487-1499`),
  and the Wayland backend sustains its frame-callback loop unconditionally
  (`gpui_linux/.../wayland/window.rs:587-602`, `client.rs:1154-1173`).
  Today's timer+notify design rides the unthrottled path — keep it.

**Build profile**: `Cargo.toml` has no `[profile]` section; `just run` is
dev-profile `cargo run` (`Justfile:24-25`). Every perception test ever run
was an unoptimized GPUI/blade/taffy/rustfft build. Zed's own workspace
ships 18+ `opt-level = 3` dev-package overrides (checkout root
`Cargo.toml:896-973`) — they don't run GPUI unoptimized either.

Conventions: `eprintln!` is the daemon's logging voice; constants at the
top of the file they govern; `just fmt` (nightly rustfmt) before finishing.

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| Live run  | `just run daemon` (Wayland + mic required)  | daemon ready line   |
| Record    | `dictate record toggle` (second terminal)   | overlay appears     |
| Control workspace | `jj workspace add ../dictate-control -r 0719b7d8` | second checkout |
| Refresh rate | `niri msg outputs` | shows Hz of the active output |

## Scope

**In scope**:
- `src/overlay.rs` (frame pacing, easing/smoothing, temporary instrumentation)
- `src/mic.rs` (cpal buffer-size request, temporary instrumentation)
- `src/spectrum.rs` (analyzer-side smoothing if Step 5 restores it;
  instrumentation hooks)
- `Cargo.toml` (dev-profile package overrides)
- A throwaway `jj` workspace at `0719b7d8` for the control run (removed at
  the end)

**Out of scope** (do NOT touch):
- `src/daemon.rs`, `src/dictation.rs`, `src/text.rs`,
  `src/transcription.rs`, `src/models.rs` — owned by plans 002–005.
- Reverting to the two-process architecture. The in-process design is the
  intended end state; the control run is evidence, not the destination.
- Forking or patching GPUI. If measurements point inside GPUI or the
  compositor, that is a STOP condition with evidence.
- Spectrum band aesthetics (gate thresholds, boosts, bar styling).

## Steps

### Step 1: Make dev builds representative

Add a dev-profile override so GPUI and the DSP path get real codegen even
in dev (precedent: Zed's `[profile.dev.package]` block):

```toml
[profile.dev.package."*"]
opt-level = 2
```

Expect a one-time full dependency rebuild. All measurements below run this
profile (cross-check any surprising number against `--release`).

**Verify**: `just check` → exit 0.

### Step 2: Add temporary cadence instrumentation

Throttled (once-per-second) `eprintln!` summaries — never a line per
event, and **no I/O or allocation inside the cpal callback** (realtime
thread; aggregate into atomics there, report from the worker). Three
points:

1. **Audio bursts** (`src/mic.rs`): callbacks/sec, mean samples/callback,
   max gap. Reveals the PipeWire quantum.
2. **Spectrum updates** (`audio_worker` at the `send_spectrum` call):
   updates/sec, max gap. This is the *content* rate.
3. **Render cadence** (`src/overlay.rs::render`): renders/sec, max
   inter-render gap while visible. The truth about what GPUI draws.

Greppable shape: `perf mic: 47 cb/s, 341 avg samples, 24ms max gap`.
Temporary — removed in Step 7.

**Verify**: `just check` → exit 0; `just test` → all pass.

### Step 3: Measure the regressed state (baseline)

Live: `just run daemon`, record, speak ~10 seconds. Capture all three
perf lines plus the display refresh rate (`niri msg outputs`). This is the
"broken" column of the results table.

**Verify**: a baseline row of numbers exists.

### Step 4: Measure the controls

Two cheap experiments that decide where the remaining problem lives:

- **Control A — the smooth build**: `jj workspace add ../dictate-control
  -r 0719b7d8`, and in that workspace add the *render-cadence counter
  only* (a ~5-line patch to its `src/overlay.rs::render`; its
  architecture differs — old `Overlay` view, `src/overlay.rs:12-37` at
  that revision). Build and run it (`cargo run -- daemon` in the
  workspace; same dev-profile override may be added there for parity).
  Record renders/sec and the subjective feel. This is the target.
- **Control B — current code, data flow muted**: in the working copy,
  temporarily skip the `send_spectrum` call (one commented line in
  `audio_worker`). Run and record renders/sec. This isolates the render
  loop from data effects *inside the cohabiting process*:
  - Control B renders/sec ≈ refresh rate → the in-process GPUI loop is
    healthy; the problem is **area A (data)**. Proceed to Step 5.
  - Control B materially below refresh rate (< ~90%) → **area C** is
    real: bisect by thread (e.g. temporarily skip recognizer setup /
    socket thread) to find the cohabitant; if nothing in this codebase
    explains it, STOP with the numbers.

Restore the muted line before continuing.

**Verify**: a three-row results table (baseline, control A, control B)
exists with renders/sec for each.

### Step 5: Fix what the numbers indict

Apply only the branches the measurements justify, re-measuring after each:

- **Content cadence bursty** (spectrum max gap ≥ ~20ms while speaking):
  request an explicit callback size in `src/mic.rs` — set
  `buffer_size: BufferSize::Fixed(n)` on the `StreamConfig`, `n` ≈ 16ms of
  input-rate audio (256 frames at 16kHz), clamped to the device's
  `SupportedBufferSize` range. If the backend rejects `Fixed`, fall back
  to `Default` and record it.
- **Motion smoothing**: the smooth build's look was ~31 evenly-spaced
  frames/sec passed through analyzer EMA 0.7 — equivalent to a time
  constant of roughly τ ≈ 90ms (per-frame keep 0.7 at 32ms/frame). The
  current view easing (`RISE_SPEED = 90` ⇒ τ ≈ 11ms) is ~8× snappier and
  reads as stepping. Restore smoothing of comparable strength — either
  time-based EMA in the analyzer or slower view-easing constants (rise τ
  in the 30–90ms range, fall slower) — tuned live against control A's
  feel. One smoothing stage total; don't reintroduce the
  double-smoothing the session already removed once.
- **Render cadence below refresh** (baseline renders/sec materially below
  control B's): the 16ms wall-clock timer beats against the compositor
  frame clock (each tick also hops background-timer → main thread before
  `notify`). Shorten `FRAME_INTERVAL` to ~6–8ms so the window is dirty
  for every frame callback; the scene is 8 tiny divs, drawing every
  callback is cheap. **Do NOT switch to
  `request_animation_frame`/`with_animation`** — the ~30fps inactive-window
  cap (Current state) makes that a trap for this never-focusable window.
  Leave a short code comment on the timer loop stating the constraint,
  with the gpui `window.rs` reference — it is invisible from this
  codebase alone.

**Verify**: after each change, the targeted number improves in a
re-measurement; `just test` → all pass.

### Step 6: Acceptance (live, against the control)

With instrumentation still in place, a ~10-second speaking session shows:

- renders/sec ≥ ~90% of display refresh rate, no inter-render gap > 35ms
  while visible;
- spectrum updates mean interval ≤ ~16ms with no gaps > ~25ms while
  speaking (or smoothing demonstrably bridges the gaps);
- the maintainer judges it **at least as smooth as control A** running
  side-by-side on the same machine.

If the numbers are green but it still feels worse than control A, STOP.

### Step 7: Clean up, record numbers

Remove the instrumentation and the control-B mute if any trace remains;
`jj workspace forget` the control workspace and delete its directory. Put
the full results table (baseline / control A / control B / after-fix) in
the PR description. `just fmt`.

**Verify**: `rg -n "perf " src/` → nothing; `just test` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0; `jj workspace list`
→ only the default workspace.

## Done criteria

- [ ] `just test` → all pass; `cargo clippy --all-targets -- -D warnings` → exit 0
- [ ] `Cargo.toml` contains the dev-profile package override
- [ ] Results table (baseline, control A, control B, after) in the PR description
- [ ] Step 6 acceptance met, including the side-by-side judgment against `0719b7d8`
- [ ] Code comment at the overlay frame loop documenting the GPUI
      inactive-window 30fps cap (with the gpui `window.rs` reference)
- [ ] No instrumentation left in `src/`; control workspace removed; only
      in-scope files modified (`jj st`)

## STOP conditions

Stop if:

- The code at the "Current state" locations doesn't match the excerpts,
  or the pinned gpui rev in `Cargo.toml:9-10` has changed (the cap
  analysis was read at rev `50d001f` — re-verify before relying on it).
- **Control B renders/sec is far below refresh rate and no cohabiting
  thread in this codebase explains it** — the bottleneck is GPUI's Wayland
  backend or the compositor's frame-callback pacing for layer-shell
  surfaces. Handback with all measured rates; the maintainer's options are
  an upstream issue/patch or accepting the rate.
- **Control A is NOT smooth on today's machine** — the "it was smooth
  before" premise fails under measurement, meaning the environment
  (compositor version, PipeWire config, driver) changed too. Handback
  with both sets of numbers; the investigation becomes environmental.
- The cpal `BufferSize::Fixed` request errors AND burstiness is the
  dominant measured problem — handback with the device's supported range.
- Numbers green but feel still worse than control A (Step 6) — describe
  what the eye sees versus what the numbers say; don't tune aesthetics
  on your own.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- The GPUI inactive-window cap (`window.rs:1436-1449` at rev `50d001f`) is
  the load-bearing external constraint on any future "use the framework's
  animation API" refactor; worth an upstream issue — a never-focusable
  layer-shell overlay is arguably a case the energy heuristic shouldn't
  throttle.
- Plan 005 makes capture session-scoped and gives `audio_worker` an exit
  condition; the `BufferSize::Fixed` request and any analyzer smoothing
  added here live in code 005 restructures — they must survive that
  refactor.
- The 8 relaxed per-band atomics can tear (a render can mix bands from two
  FFT frames). Judged imperceptible at 8ms hops; revisit only if shimmer
  remains after pacing is fixed.
- The pre-regression architecture (`jj file show -r 0719b7d8 src/app.rs`)
  remains the reference implementation of "what smooth meant" — including
  its even ~31Hz content cadence and EMA 0.7 — if future tuning drifts.
