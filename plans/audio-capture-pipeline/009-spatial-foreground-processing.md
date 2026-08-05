# Plan 009: Compare native stereo foreground transforms without changing production capture

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback. Do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from e63dfe42 --to @ -- crates/dictate-speech/src/mic.rs crates/dictate-speech/src/eval.rs crates/dictate-speech/src/lib.rs crates/dictate-dev/src crates/dictate/src/cli.rs`
> This plan was written while the larger audio experiment remained in working-copy change `kwllkklu`. Reconcile any drift with the current excerpts below before editing. Stop if another change has altered the capture channel model or dev-command routing.

## Status

- **Status**: DONE — no-go; fixed mid/side cancellation fails controlled acoustic calibration
- **Effort**: M
- **Risk**: MEDIUM
- **Depends on**: Plans 001 and 004
- **Planned at**: working-copy revision `e63dfe42`, 2026-08-05

## Why this matters

The laptop exposes two distinct microphone channels, but Dictate averages them before any spatial evidence can be used. Existing controlled recordings contain user-only, TV-only, and mixed dialogue in native stereo. A bounded offline comparison can show whether channel selection, mid/side processing, or delay-and-sum beamforming lowers television insertions without harming nearby or quiet speech.

This plan is an experiment. It must not change ordinary microphone capture, default downmixing, routing, or WAV transcription.

## Current state

- `crates/dictate-speech/src/mic.rs::build_input_stream` converts negotiated input samples and averages all channels before resampling and delivery to `CaptureHandler`.
- `crates/dictate-speech/src/eval.rs` owns transcript normalization and WER/CER measurement used by the degradation matrix.
- `crates/dictate-dev/src/lib.rs` and `crates/dictate/src/cli.rs` route dev-only headless commands.
- `/tmp/dictate-channel-probe/user-only.wav`, `tv-only.wav`, and `mixed-dialogue.wav` are the consented native 48 kHz stereo diagnostic files on the planning machine. `mixed.wav` is invalid evidence because it contains no TV dialogue.
- The current dev capture bank saves the post-resample 16 kHz mono utterance. Those files cannot support this experiment.

Follow the fail-open pattern in `crates/dictate-speech/src/mic.rs::EchoPipeline::emit_pending_raw`: an uncertain transform never replaces the known-good input.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format | `just fmt` | exit 0 |
| Check | `just check` | exit 0 |
| Unit tests | `just test` | all pass |
| Model integration | `just test-integration` | all pass |
| Lint | `just lint` | exit 0 |
| Architecture | `just hawk` | zero findings |

## Scope

**In scope**:

- `crates/dictate-speech/src/spatial.rs` — offline, typed stereo transforms and metrics.
- `crates/dictate-speech/src/lib.rs` — export only the dev probe interface needed by `dictate-dev`.
- `crates/dictate-speech/src/eval.rs` — reuse or extend transcript metrics without changing existing rows.
- `crates/dictate-dev/src/spatial_probe.rs` — machine-readable A/B runner.
- `crates/dictate-dev/src/lib.rs` — dev command routing.
- `crates/dictate/src/cli.rs` — dev-only CLI arguments.
- This plan, the effort README, and a spatial-results memo.

**Out of scope**:

- `crates/dictate-speech/src/mic.rs` production downmixing — no transform ships from this experiment.
- `crates/dictate-speech/src/speaker.rs` — identity and diarization cannot select the winning spatial output.
- `crates/dictate-speech/src/echo.rs` — playback AEC is Plan 010.
- Native stereo capture during normal dictation — add it only after this experiment finds a useful transform.
- A corpus manager, manifest format, or automatic sample labeling.
- Persistent audio routing, virtual PipeWire devices, and target-speaker extraction.

## Steps

### Step 1: Add a typed native stereo probe input

Add an owned diagnostic type that records sample rate, exactly two named channels, and finite interleaved or deinterleaved samples. Keep it separate from `CapturedUtterance`, whose invariant is 16 kHz mono dictation audio.

Load PCM or IEEE-float stereo WAVs for the dev probe. Reject mono, more than two channels, empty audio, mismatched channel lengths, invalid rates, and non-finite samples with specific errors.

**Verify**: `cargo test -p dictate-speech spatial` → loader and invariant tests pass.

### Step 2: Implement bounded comparison variants

Produce independent mono outputs for:

1. Current equal channel average.
2. Left channel only.
3. Right channel only.
4. Mid and side measurements, including polarity-safe side energy diagnostics.
5. Delay-and-sum outputs across a small declared delay range supported by the sample rate and plausible laptop microphone spacing.

Every output must preserve duration within one source frame, remain finite, and use the existing bandlimited resampler when converted to 16 kHz. Report channel RMS, correlation, relative lag, and each variant's parameters. Do not use an ASR transcript to choose or tune a steering delay.

**Verify**: `cargo test -p dictate-speech spatial` → identical-channel, known-delay, polarity, silence, duration, and resampling tests pass.

### Step 3: Add a headless dev-only A/B command

Add `dictate-dev spatial-probe <stereo-wav>` with optional target and interferer reference text. Emit one JSON document containing source metrics and one row per variant:

- transform name and parameters;
- output signal metrics;
- raw transcript or typed transcription failure;
- target WER when a target reference is supplied;
- interferer insertion evidence when an interferer reference is supplied.

A failed variant must not hide successful variants or overwrite source audio. Ordinary `dictate transcribe <wav>` must remain unchanged.

**Verify**: invoke the command on a generated deterministic stereo test file → JSON parses and includes every variant.

### Step 4: Run the existing controlled comparison

Run the command against `user-only.wav`, `tv-only.wav`, and `mixed-dialogue.wav`. Record exact commands and per-variant results in `memo-spatial-foreground-results.md`.

Judge variants by language outcome:

- nearby-user transcript retention;
- quiet-user retention where evidence exists;
- TV-only transcript suppression;
- TV insertion in the valid mixed-dialogue clip;
- stability of the chosen steering direction across files.

Do not choose a production transform from RMS, listening, one attractive transcript, or an ASR-oracle parameter search.

**Verify**: the memo contains the baseline and every variant for all three valid clips, plus a ship/no-ship verdict.

### Step 5: Stop at the first-sweep evidence gate

The declared channel, raw-side, and integer-delay variants produced no shippable winner. Keep production averaging. The results and corrected WER interpretation are recorded in `memo-spatial-foreground-results.md`.

**Verify**: `just fmt && just check && just test && just test-integration && just lint && just hawk` → all pass.

### Step 6: Add a content score for spatial evaluation

Keep literal WER unchanged for compatibility with the existing evaluation harness. Add a separate spatial content score that canonicalizes equivalent number forms and acronym tokenization such as `TV` versus `T V`. New spoken scripts must avoid cardinals, ordinals, and ambiguous acronyms where possible. Record both scores; never hide literal output.

**Result**: implemented. Existing literal scores remain visible; the added content score treats the first sweep's digit and acronym renderings as equivalent while retaining lexical substitutions.

**Verify**: focused tests cover spelled numbers, digits, joined digits, and acronym tokenization without weakening ordinary lexical substitutions.

### Step 7: Compare constrained mid/side cancellation

Keep the ordinary mid signal as the target path and use side only as a bounded cancellation reference:

```text
mid = (left + right) / 2
side = (left - right) / 2
output = mid - H(f) × side
```

Estimate fractional timing and frequency weights from channel phase or isolated acoustic measurements, not transcript content. Preserve unity gain for centered speech, pass through bands where the array lacks spatial resolution, cap noise gain, and retain raw mid as fallback. Lock every parameter before ASR comparison. Do not change `mic.rs`.

**Final result**: the dev probe fits bounded complex weights, reports the frozen fit, validates it acoustically before ASR, and falls back explicitly to average on failure. A 250 Hz pooled fit retained only three high-frequency bands. A per-FFT-bin refinement then fit the first half of `tv-only.wav` and validated on the second half; it found zero bins that met both the 0.80 fit-coherence gate and 3 dB validation-reduction gate.

Consented fixed-geometry follow-up recordings used silence and continuous mono pink noise. The production digital-microphone path reduced the steady pink-noise capture by about 21 dB between the first and second ten-second halves. A repeat fell by about 20 dB, and an eight-second run fell from -26.2 dBFS in its first four seconds to -50.4 dBFS in its last four. The fit again found zero usable bins. Since the observed input path is time-varying even with fixed source, level, and geometry, it cannot supply the stable transfer required by a frozen canceller. Do not lower either acoustic gate or select bins from ASR output.

**Verify**: synthetic centered-target and off-axis-interferer tests prove target-gain bounds, finite duration-preserving output, low/high-band passthrough, bounded noise gain, deterministic parameter selection, and rejection when the held-out transfer changes.

### Step 8: Run a held-out language gate

Use independent target and TV references. The final set needs several mixed clips whose average baseline contains TV insertions; do not record them without explaining the script and receiving explicit `go` or `ready`. Fit or calibrate on separate isolated clips, freeze parameters, then evaluate the held-out mixed clips plus nearby and quiet user clips.

A candidate advances only if it lowers interferer insertion on held-out mixtures without worsening canonical target content. Otherwise close Plan 009 with a no-go verdict. Any production capture change still requires a separate plan.

**Result**: no candidate reached this gate. Controlled mono calibration failed the pre-ASR acoustic holdout, so running language evaluation on the exact-average fallback would add no evidence. Plan 009 closes with a no-go verdict and leaves production averaging unchanged.

**Verify**: the results memo records the calibration inputs, acoustic rejection, reason the language gate did not run, and final no-ship verdict.

## Test plan

- Stereo WAV loading rejects invalid shapes and values.
- Equal channels reproduce the current mono average.
- Synthetic delayed channels recover the known lag.
- Delay-and-sum variants preserve duration and finite sample bounds.
- Silence and opposite-polarity channels do not produce NaN or division by zero.
- JSON retains failures beside successful rows.
- Existing degradation and ordinary WAV transcription tests remain unchanged.

## Done criteria

- [x] A machine-readable dev command compares the declared stereo variants.
- [x] Existing controlled stereo clips have complete per-variant transcript evidence.
- [x] The first-sweep memo gives a no-ship verdict for channel, raw-side, and integer-delay variants without changing production capture.
- [x] The first-sweep `just fmt && just check && just test && just test-integration && just lint && just hawk` gate passes.
- [x] No first-sweep files outside the in-scope list are modified.
- [x] Spatial evaluation reports canonical content score beside literal WER.
- [x] Constrained cancellation preserves centered target gain under synthetic tests.
- [x] Controlled mono calibration resolves the frozen candidate at the pre-ASR acoustic gate.
- [x] A final no-ship verdict closes Plan 009.

## STOP conditions

Stop and write a handback if:

- The valid stereo clips are missing. Do not begin a new recording without explaining exactly what will be recorded and receiving explicit `go` or `ready`.
- The channels are duplicate, direction evidence changes unpredictably, or no bounded delay range is physically defensible.
- A candidate improves TV suppression but deletes or worsens nearby or quiet user speech.
- Selecting a transform requires transcript content, speaker identity, or a threshold tuned to one clip.
- The work requires production capture changes, persistent routing, or another inference runtime.
- A verification command fails twice after a reasonable fix.

The handback must describe current evidence, the desired outcome, and the unresolved fork without choosing a branch.

## Maintenance notes

Keep spatial transforms independent from AEC, diarization, and speaker identity. This plan produced no viable transform. Reopening spatial cancellation requires a new plan with a stable raw stereo source or an explicitly time-varying method; it must not reuse this fixed calibration by weakening its gates.
