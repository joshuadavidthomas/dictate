# GPUI rewrite hardening

Fixes for the highest-leverage findings from the 2026-06-11 `/improve` audit
of the GPUI-native rewrite (branch `gpui-native-rewrite`), planned at
revision `mtnsrkmyruyz` (git `dd6db2c175a3`) — note this was the
**working-copy snapshot**, with uncommitted changes on top of commit
`0719b7d8`; the drift checks in each plan account for it. Three tracks: 001
(CI) and 002 (formatter) are independent of everything; 003 → 004 → 005 all
touch the daemon/mic files and must run in that order; 006 (overlay FPS)
is logically independent but shares `src/mic.rs` with the daemon track and
should run before it.

Execute in the order below unless dependencies say otherwise. Each executor:
read the plan fully before starting, honor its STOP conditions, and update
your row when done.

## Execution order & status

| Plan | Title | Effort | Depends on | Status |
|------|-------|--------|------------|--------|
| [001](001-rust-ci.md) | Replace Tauri-era CI with a Rust CI pipeline | M | — | DONE |
| [002](002-formatter-asr-punctuation.md) | Make the formatter punctuation-safe on real ASR output | M | — | DONE |
| [003](003-daemon-resilience.md) | Harden the daemon against hangs and zombie states | M | 001 | DONE |
| [004](004-bounded-recording.md) | Bound recording length; document Whisper 30s limit | S–M | 003 | DONE |
| [005](005-idle-mic-release.md) | Release the microphone while idle | M | 003, 004 | DONE |
| [006](006-overlay-frame-pacing.md) | Fix the overlay's choppy, low-FPS spectrum animation | M | — (file conflict with 003–005) | DONE |

Status values: TODO | IN PROGRESS | DONE | BLOCKED (one-line reason) |
SUPERSEDED (one-line pointer to what replaced it)

## Dependency notes

- **001 first**: it restores a CI gate (`check`/`test`/`clippy`/`fmt`) that
  every later PR lands behind.
- **003 → 004 → 005**: all modify `src/daemon.rs`/`src/mic.rs`. 005
  restructures the capture lifecycle and must preserve 003's error-callback
  wiring and 004's recording cap; running it first would force the others to
  re-plan.
- **002** is isolated to `src/text.rs` and can run any time, in parallel
  with the daemon track.
- **006** has no logical dependencies but touches `src/mic.rs` like
  003/004/005 — never run it concurrently with the daemon track. It
  addresses the maintainer's most painful issue (overlay perceived ~30fps);
  running it **first**, before the daemon track, is recommended.

## Reconciliation log

- **2026-06-11**: Effort created from the `/improve` audit (5 plans).
  Next: 001.
- **2026-06-11**: Added 006 from a focused `/improve perf` audit of the
  overlay FPS complaint (sourced from the 2026-06-04 pi debugging session
  plus a read of the pinned GPUI rev `50d001f`). Recommended order now:
  006 → 001 → 002 ∥ (003 → 004 → 005).
- **2026-06-11 (later)**: 006 rewritten after pinpointing the regression in
  jj history: the smooth state is commit `0719b7d8` (two-process design,
  FFT in the cpal callback at an even ~31 frames/sec with analyzer EMA 0.7);
  the uncommitted in-process merge simultaneously introduced 4096-sample
  worker batching (256ms ≈ the observed ~2–4fps collapse). 006 now measures
  against `0719b7d8` as a control and investigates three areas: data
  production cadence, transport, render pacing/process cohabitation.

## Considered and rejected

(So nobody re-audits these.)

- **Replace `VecDeque` in `DictationControlState::Transcribing` with a
  single `Option`** (`src/dictation.rs:230-232`, queue can never exceed 1):
  real but cosmetic; not worth a plan, fold into any future touch of the file.
- **Unify `DictationCommand`'s `FromStr` with its serde representation**
  (`src/dictation.rs:43-55` duplicates the wire strings): drift risk is
  covered by the round-trip test at `src/daemon.rs:206-218`; cosmetic.
- **Delete dead `VadModel`** (`src/models.rs:472-491`, zero references):
  PLAN.md explicitly reserves VAD for the continuous path; harmless to keep
  briefly, trivial to delete in passing.
- **Anti-aliasing filter for `LinearResampler`** (`src/mic.rs:166-220`):
  the resampler only engages on the fallback path — `input_config`
  (`src/mic.rs:79-99`) requests native 16kHz first, which PipeWire devices
  generally honor; revisit only if ASR quality complaints surface on 48kHz-only
  hardware.
- **Checksum verification of model downloads** (`src/models.rs:137-168`):
  HTTPS from GitHub releases; sherpa-onnx publishes no stable per-archive
  checksum manifest to pin against. Revisit if a release/packaging story
  needs reproducibility.
- **Same-user socket DoS as a security finding** (`src/daemon.rs`):
  `$XDG_RUNTIME_DIR` is mode-0700 per-user; only the user can wedge their own
  daemon. Treated as a robustness bug (read timeout, plan 003), not security.
- **Switching overlay animation to `request_animation_frame`/`with_animation`**
  (the "GPUI-native" pattern): GPUI hard-caps `next_frame_callbacks`-driven
  frames at ~30fps on non-active windows (gpui `window.rs:1436-1449` at rev
  `50d001f`), and the layer-shell overlay (`KeyboardInteractivity::None`)
  can never become active. This is why the 2026-06-04 session's
  framework-native attempts plateaued at ~15–25fps. Plan 006 documents the
  trap in code instead.
- **Per-band atomic tearing in `SpectrumLevels`** (`src/spectrum.rs:27-54`,
  8 relaxed `AtomicU32`s can mix two FFT frames in one paint): judged
  imperceptible at 8ms hops; noted in plan 006's maintenance notes,
  revisit only if shimmer remains after pacing is fixed.
- **Audio-worker micro-tuning as an FPS fix** (`WORKER_BATCH_SAMPLES`,
  `EMPTY_RING_SLEEP`, allocation churn in `src/mic.rs`): all tried during
  the 2026-06-04 session with no measured effect; the allocation purge
  already landed. Don't re-tune these blind — plan 006's instrumentation
  decides.
- **Blaming the GPUI notify loop for the regression**: the smooth
  pre-regression overlay (`0719b7d8`) used the *identical* 16ms timer +
  `cx.notify()` render mechanism. The render loop cannot be the sole cause;
  plan 006's controls discriminate data cadence from process cohabitation.
- **Reverting to the two-process overlay architecture**: the in-process
  design is the intended end state (single binary, no pipe protocol);
  `0719b7d8` serves as a measurement control in plan 006, not a
  destination.

## Deferred

(Real, but not planned in this effort.)

- **Delivery targets (copy/insert)** — README:21 names this as the next
  focus; output is stdout-only today (`src/daemon.rs:128`). Product work,
  not a hardening fix; deserves its own design plan.
- **Settings/config wiring** — the entire `text.rs` mode/dictionary/
  replacement machinery and the 16-model catalog (`src/models.rs:348-365`)
  are unreachable at runtime: `src/daemon.rs:109-113` hardcodes
  `default_model()` and `DictationContext::default()`. PLAN.md lists TOML
  settings as a behavior to keep. Highest-leverage direction item alongside
  delivery.
- **Overlay phase states** — PLAN.md:206's overlay contract
  (idle/recording/transcribing/error); today only the waveform renders, and
  during transcription it keeps animating from the live mic (plan 005
  changes that to a freeze, still without a distinct "transcribing" visual).
- **Structured logging** — ~20 `eprintln!` sites; AGENTS.md says prefer
  `log`/`tracing` once wired. Worth doing before the daemon gets a systemd
  unit.
- **Long-dictation transcription (>30s)** — VAD-segmented/windowed decode on
  the continuous path; plan 004 only bounds and documents it.
- **Release/packaging automation for the GPUI binary** — old signed
  deb/AppImage/rpm + AUR pipeline deleted by plan 001; the old shape is in
  VCS history at `dd6db2c175a3` (`.github/workflows/release.yml`).
- **Worker auto-retry from `Unavailable`** — plan 003 makes the state
  visible and actionable, not self-healing.
- **Ambient mic noise / input sensitivity pass** — plan 006 fixed the overlay
  cadence regression, but the laptop mic still drives small idle spectrum
  motion. Treat that separately as a noise gate, VAD threshold, or mic gain
  calibration pass; do not fold it back into frame pacing.
