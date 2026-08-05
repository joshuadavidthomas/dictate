# Plan 010: Validate client-owned playback echo cancellation without changing audio routing

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback. Do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from e63dfe42 --to @ -- Cargo.toml Cargo.lock .github/actions/install-linux-dependencies/action.yml crates/dictate-speech/Cargo.toml crates/dictate-speech/src/echo.rs crates/dictate-speech/src/mic.rs`
> The paused WebRTC experiment remains in archived local change `kwllkklu`; it is absent from the usable build. Reconcile drift before resuming and stop if another change has altered stream ownership or fallback behavior.

## Status

- **Status**: BLOCKED — paused; prototype archived outside the usable build
- **Effort**: M–L
- **Risk**: HIGH
- **Depends on**: Plans 001, 002, 004, and 008
- **Planned at**: working-copy revision `e63dfe42`, 2026-08-05

## Why this matters

Audio played by the same computer can enter the microphone and become dictated text. WebRTC echo cancellation can use a matching playback-monitor stream, but only when render and microphone frames stay aligned. Dictate must own both streams inside its process so a crash cannot leave virtual devices, moved streams, or changed defaults behind.

This plan validates the existing experiment. It does not enable AEC by default.

## Current state

- Archived `crates/dictate-speech/src/echo.rs::EchoCancellation` wraps bundled `webrtc-audio-processing` with 160-sample frames, high-pass filtering, full echo cancellation, and noise suppression.
- `crates/dictate-speech/src/mic.rs::capture` opens the current default output monitor only when `DICTATE_EXPERIMENTAL_ECHO_CANCELLATION` is present.
- `ReferenceReader::frame` waits for a complete render frame. Underflow, dropped samples, or stream failure disables processing rather than padding and shifting the reference timeline.
- `EchoPipeline` retains pending raw microphone samples and emits them when the reference fails.
- Controlled standard-mode testing reduced laptop playback while preserving the requested phrase. Mobile mode destroyed requested speech and is rejected.
- Playback-sink selection, cross-stream timing, quiet-speech safety, release packaging, and CPU cost remain unresolved.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format | `just fmt` | exit 0 |
| Check | `just check` | exit 0 |
| Unit tests | `just test` | all pass |
| Integration | `just test-integration` | all pass |
| Lint | `just lint` | exit 0 |
| Architecture | `just hawk` | zero findings |
| Release build | `just build --release` | exit 0 |

## Scope

**In scope**:

- `Cargo.toml`, `Cargo.lock`, and `crates/dictate-speech/Cargo.toml` — bundled dependency and feature boundaries.
- `.github/actions/install-linux-dependencies/action.yml` — only dependencies required by the chosen distributable build.
- `crates/dictate-speech/src/echo.rs` — WebRTC processor adapter.
- `crates/dictate-speech/src/mic.rs` — client-owned capture/reference streams, alignment diagnostics, and raw fallback.
- A focused AEC findings memo and the effort README.

**Out of scope**:

- Spatial stereo processing — Plan 009.
- External television speech — no playback reference exists.
- PipeWire modules, virtual sinks, default-device changes, or moved application streams.
- A fixed residual amplitude gate.
- Mobile WebRTC mode.
- Enabling AEC by default.
- Speaker diarization or identity.

## Steps

### Step 1: Lock stream ownership and fail-open behavior

Characterize the existing dual-stream state transitions in tests. The microphone stream, playback-monitor stream, resamplers, and worker must be process-owned. Any reference underflow, dropped sample, format error, or processing error must produce raw microphone audio for the affected utterance and disable further AEC for that utterance.

No correctness claim may depend on `Drop`, SIGTERM cleanup, or a restoration action.

**Verify**: `cargo test -p dictate-speech echo` and `cargo test -p dictate-speech reference` → fail-open and partial-tail tests pass.

### Step 2: Add machine-readable timing diagnostics

Record enough bounded diagnostic data to explain render/capture alignment without retaining audio unless `DICTATE_CAPTURE_DIR` is also enabled. Include negotiated rates, channels, callback timestamps when available, resampler counts, queued render depth, underflow, overflow, and the point where fallback occurred.

Diagnostics must not change normal processing or create a second routing mechanism.

**Verify**: deterministic timing tests cover aligned frames, initial render delay, temporary underflow, overflow, and callback discontinuity.

### Step 3: Validate the playback-reference choice

Prove which sink monitor supplies the application playback being canceled. The current default sink monitor is acceptable only when the tested application is routed there. If PipeWire cannot expose a reliable client-owned match for applications routed to other sinks, retain the experimental gate and report the limitation. Do not move streams or change defaults to manufacture a match.

**Verify**: the findings memo records at least default-sink success and the behavior when playback uses another sink or no monitor is available.

### Step 4: Run quiet-speech and playback A/B tests

With explicit recording consent, compare raw and AEC output for:

- quiet nearby speech with no playback;
- playback with no speech;
- quiet nearby speech over playback;
- normal nearby speech over playback.

Use the same source conditions for raw and processed paths when possible. Report raw transcript parity, target-word retention, playback insertion, underflow/fallback events, wall time, and peak memory. Never choose a residual loudness threshold.

**Verify**: the memo includes exact conditions and a fail-open verdict for each row.

### Step 5: Keep the feature experimental unless every gate clears

Default enablement requires stable timing, quiet-speech parity, correct sink reference, supported-hardware release performance, and distributable bundled builds. If any gate remains open, keep `DICTATE_EXPERIMENTAL_ECHO_CANCELLATION` mandatory and document the remaining blocker.

**Verify**: `just fmt && just check && just test && just test-integration && just lint && just hawk && just build --release` → all pass.

## Test plan

- Complete render frames are never fabricated by zero padding.
- Reference failure emits every pending raw microphone sample once.
- Microphone discontinuity disables AEC for the utterance.
- Processing failure cannot produce silence or lose the partial tail.
- No environment variable means the existing single-stream path.
- Missing monitors warn and continue with raw microphone audio.
- Device pinning remains strict.

## Done criteria

- [ ] Client-owned stream and fallback invariants are covered by tests.
- [ ] Timing and sink-selection evidence is recorded.
- [ ] Quiet-speech and playback A/B rows have language outcomes.
- [ ] The default remains disabled unless every listed gate clears.
- [ ] The full verification command passes.
- [ ] No persistent audio route or PipeWire object is created.

## STOP conditions

Stop and write a handback if:

- A proposed fix creates server-owned audio state or depends on cleanup after process death.
- Reference alignment requires zero padding, dropping quiet microphone frames, or an amplitude gate.
- The requested playback stream cannot be matched without moving it.
- Quiet speech changes or disappears under AEC.
- The bundled dependency cannot be distributed on a supported Linux target.
- A requested diagnostic recording lacks explicit `go` or `ready` after its contents are explained.
- A verification command fails twice after a reasonable fix.

## Maintenance notes

AEC removes only laptop playback with a matching render reference. It must remain independent from spatial processing and speaker analysis. Future support for several output sinks should extend reference selection, not mutate system routing.
