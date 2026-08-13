# Plan 003: Make the overlay meter adapt to signal level instead of assuming a hot mic

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 8bbf8294 -- crates/dictate-signal/src/spectrum.rs`
> If `spectrum.rs` changed since planning, compare the "Current state"
> excerpts against the live code before proceeding; on a mismatch, STOP.

## Status

- **Effort**: M
- **Risk**: MED (changes the overlay's visual feel; DSP tuning involved)
- **Depends on**: none (shares no files with 001/002/004/005)
- **Planned at**: revision `nootnkmorwsk` (git `8bbf8294`), 2026-08-04

## Why this matters

The 2026-08 headset incident: valid speech at whole-recording RMS 0.002141
was captured while the overlay meter showed (at best) a barely-flickering
waveform. The meter assumes a hot microphone in two places: every band
subtracts a **fixed absolute noise floor of 0.005** — larger than that
entire recording's RMS — and the waveform stays zeroed until a band crosses
an absolute visual gate of 0.16. So on a quiet-but-clean mic, the UI tells
the user "I hear nothing" while capture works fine. This was the visual half
of the same absolute-amplitude disease whose gating half (the RMS discard
gate) was already removed. After this plan: speech on a quiet mic visibly
drives the meter; digital silence still gates to a flat line.

## Current state

- `crates/dictate-signal/src/spectrum.rs` is pure DSP (no GPUI) — fully unit
  testable. The daemon feeds it 16 kHz mono samples via
  `SpectrumAnalyzer::push_sample` (called from the mic worker,
  `crates/dictate-speech/src/mic.rs:363-367`), and the overlay renders the
  smoothed bands.
- `spectrum.rs:223-251` — `SpectrumBand::level`: per-band FFT RMS, then

  ```rust
  let noise_floor = 0.005;                       // spectrum.rs:242
  let signal = (rms - noise_floor).max(0.0);     // zeroes quiet mics
  let compressed = (signal * self.display_boost).sqrt();
  // then an absolute per-band gate_threshold, then normalize
  ```

- `spectrum.rs:19-23` — `DEFAULT_WAVEFORM_SMOOTHING` with `visual_gate_on:
  0.16`, `visual_gate_off: 0.08`; applied in `advance_waveform_bands`
  (`spectrum.rs:97-137`): until the band peak crosses `visual_gate_on`, the
  displayed bands are forced to zero (hysteresis via `visual_gate_off`).
- `VISUAL_BANDS` entries carry per-band `display_boost` and `gate_threshold`
  constants (`spectrum.rs:205-221`).
- Existing test pattern: `mod tests` at the bottom of `spectrum.rs`
  (e.g. `waveform_gate_uses_on_and_off_thresholds`, spectrum.rs:258+).
- **Invariant**: this path feeds *visuals only*. ASR samples flow through
  `CaptureHandler::samples` → `record_samples` before the spectrum analyzer
  ever sees them (`mic.rs:360-368`); nothing here may touch that.
- Visual inspection affordance: `just debug-eval` drives the debug overlay
  screen headlessly (`dictate-dev`, scenario `recording-sine`); the debug
  window embeds the production overlay component (project rule: debug
  previews reuse real seams — see `AGENTS.md`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Unit tests | `cargo test -p dictate-signal` | all pass |
| Typecheck | `just check` | exit 0 |
| Lint | `just lint` | exit 0 |
| Debug overlay smoke | `just debug-eval` | jq pipeline exits 0 |

## Scope

**In scope** (the only files you should modify):
- `crates/dictate-signal/src/spectrum.rs`

**Out of scope**:
- `crates/dictate-speech/src/mic.rs` — the worker's call pattern
  (`push_sample` per sample) must keep working unchanged; plans 002/004 own
  that file.
- `crates/dictate-ui/**`, `crates/dictate-dev/**` — the overlay consumes
  whatever `dictate-signal` emits; if renders need changes, that's a STOP.
- The ASR path — nothing in `record_samples`' input may change.

## Steps

### Step 1: Replace the absolute floor with an adaptive noise-floor estimate

Intent, with implementation latitude:

- Track a running noise-floor estimate per band (or one global scalar —
  executor's call) from the band RMS stream: a decaying-minimum or
  low-percentile tracker is the standard shape (fast attack downward, slow
  release upward, so speech onsets don't inflate the floor).
- `signal` becomes energy **relative to the estimated floor** (a ratio or
  dB-above-floor), replacing `rms - 0.005`.
- The visual gates become relative to the same scale, replacing the absolute
  0.16/0.08 pair while keeping the hysteresis structure in
  `advance_waveform_bands` (the on-gate must stay above the off-gate).
- Keep the config-struct shape (`WaveformSmoothingConfig`, `SpectrumBand`)
  so callers don't change; renaming fields to reflect the new semantics is
  encouraged, constants stay in this file.

State to carry across frames lives naturally in `SpectrumAnalyzer` (it is
already stateful — `sample_buffer`). `advance_waveform_bands` is a free
function on plain data; if floor state must reach it, extend its config or
band inputs rather than adding globals.

### Step 2: Acceptance tests (write these against real signal shapes)

In `spectrum.rs`'s `mod tests`, using synthesized 16 kHz input:

1. **Quiet speech drives the meter**: an amplitude-modulated multi-tone
   (e.g. 200 Hz + 1 kHz + 2.5 kHz, modulated at ~4 Hz) scaled to peak
   amplitude ≈ 0.003 — the headset case — must open the visual gate within
   the first second of frames and produce nonzero smoothed bands.
2. **Digital silence stays flat**: all-zero input never opens the gate.
3. **Low-level constant noise stays flat**: unmodulated uniform noise at
   amplitude ≈ 0.0005 must not open the gate once the floor has adapted
   (allow a brief startup transient; assert the steady state).
4. **Loud speech unchanged in spirit**: the same multi-tone at amplitude 0.3
   opens the gate — no regression for normal mics.

These four are the machine-checkable definition of "adaptive"; tune the DSP
until they pass honestly (no threshold set so loose that test 3 fails).

**Verify**: `cargo test -p dictate-signal` → all pass.

### Step 3: Visual sanity check

**Verify**: `just debug-eval` → exit 0 (frame pacing unaffected). Then note
in your completion summary that the maintainer should eyeball the overlay
(`dictate-dev` debug screen, scenario `recording-sine`) before merge — feel
is a taste judgment the tests can't make.

## Test plan

Step 2 is the test plan. Structural pattern: existing tests in
`spectrum.rs:254+`. All tests pure-Rust, no model, no audio hardware.

- **Verify**: `cargo test -p dictate-signal` → all pass, including the four
  acceptance tests.

## Done criteria

- [ ] `cargo test -p dictate-signal` → all pass
- [ ] No absolute amplitude constant remains load-bearing for gating
      (grep: `0.005`, `0.16`, `0.08` in `spectrum.rs` either gone or
      re-derived as relative quantities with a comment)
- [ ] `just check` → exit 0; `just lint` → exit 0; `just debug-eval` → exit 0
- [ ] No files outside `crates/dictate-signal/src/spectrum.rs` modified

## STOP conditions

Stop and write a handback if:

- Making the gate adaptive requires changing the `dictate-ui` overlay or the
  `mic.rs` call pattern — the seam assumption failed.
- Tests 1 and 3 can't both pass with one parameterization — the
  quiet-speech vs. constant-noise discrimination needs a design decision
  (e.g. modulation-aware gating) beyond this plan's intent.
- Anything requires touching the samples delivered to `record_samples`.
- Current-state excerpts don't match the live code.

## Maintenance notes

- Plan 006 (VAD spike) may eventually give the UI a *speech* signal, which
  could replace energy gating entirely; this plan's adaptive floor is still
  the right default when no VAD model is present.
- Reviewer attention: the floor tracker's release rate — too fast and
  sustained speech gets absorbed into the floor mid-utterance (meter dies
  while talking); test 1's modulation is designed to catch this, but eyeball
  it too.
- Deferred: a dedicated quiet-mic debug scenario in `dictate-dev` (would be
  nice; per project rule it must drive the real component through real
  seams).
