# Audio capture pipeline

Fixes and investigations from the 2026-08-04 `/improve` audit of the audio
capture path, prompted by a real incident: a normal headset's speech (RMS
0.002141, −53 dBFS) was captured correctly and then discarded by a fixed
amplitude gate, with the overlay meter showing near-silence throughout. The
audit's thesis: **level-invariance must be a property of the whole pipeline**
— capture format, gating, metering, and evaluation — not a patch at one
point, and every "should we add pipeline stage X" question (VAD, denoising)
gets decided by WER measurement, not by imitating meeting software.

Planned at revision `nootnkmorwsk` (git `8bbf8294`, "prefer precise capture
formats and stop discarding quiet dictation") — the commit containing the
capture-format preference fix and RMS-gate removal from the incident
debugging session; those changes are vetted and assumed present.

Two tracks: 001–005 are build plans (001 first — it is the measurement
substrate the spikes need and the lock on the incident itself); 006–007 are
measurement spikes that produce memos, not code; 008 is an independent
feature that shares files with 002/004/005 and runs last.

Execute in the order below unless dependencies say otherwise. Each executor:
read the plan fully before starting, honor its STOP conditions, and update
your row when done.

## Execution order & status

| Plan | Title | Effort | Depends on | Status |
|------|-------|--------|------------|--------|
| [001](001-degradation-eval-matrix.md) | Extend the WER harness with a degradation matrix | M | — | DONE |
| [002](002-bandlimited-resampler.md) | Replace the fallback resampler with sherpa's bandlimited one | S–M | — | DONE |
| [003](003-adaptive-overlay-meter.md) | Make the overlay meter adapt to signal level | M | — | DONE |
| [004](004-capture-diagnostics.md) | Capture diagnostics: device name + persistable audio | S | — (after 002: file overlap) | DONE |
| [005](005-honest-recording-overlay.md) | Show "Recording" only when the mic is live | M | — (after 004: file overlap) | DONE |
| [006](006-vad-spike.md) | Spike: Silero VAD as the speech front-end | M–L | 001 | TODO |
| [007](007-denoiser-ab-spike.md) | Spike: denoiser A/B against raw ASR | S | 001 | TODO |
| [008](008-input-device-selection.md) | Input device selection (setting + `devices` CLI) | M | — (after 002, 004, 005: file overlap) | TODO |

Status values: TODO | IN PROGRESS | DONE | BLOCKED (one-line reason) |
SUPERSEDED (one-line pointer to what replaced it)

## Dependency notes

- **001 → 006/007**: the spikes are measurement exercises; without the
  degradation rows they'd be opinions. 001's quiet rows are also the
  regression lock on the incident.
- **002 → 004 → 005 → 008 (soft, file-overlap only)**: 002 and 004 touch
  `crates/dictate-speech/src/mic.rs`; 004 and 005 touch
  `crates/dictate/src/daemon.rs`; 008 touches both plus settings/CLI.
  The order avoids conflict churn, not logical coupling.
- **003** is isolated in `crates/dictate-signal` and can run any time, in
  parallel with everything.
- **006 + 007 outcomes converge**: if both recommend shipping, the follow-up
  build plan must decide pipeline ordering (VAD before/after denoise) — write
  it only after both memos exist.

## Reconciliation log

- **2026-08-04**: Plan 005 implemented. The overlay shows an inactive opening state until the current recording's microphone opens, then switches to the live waveform; successful opens log their latency.
- **2026-08-04**: Plan 004 implemented. Capture startup logs the selected device, and `DICTATE_CAPTURE_DIR` saves post-resample utterances as replayable 16-bit mono WAV files without replacing earlier captures.
- **2026-08-04**: Plan 003 implemented. The overlay spectrum now measures each band relative to an adaptive floor, keeps sustained structured signals visible while still adapting to stationary broadband and tonal noise, and gates silence flat.
- **2026-08-04**: Plan 002 implemented. The fallback capture path now uses sherpa-onnx's bandlimited streaming resampler, including an ordered final flush; the 12 kHz aliasing probe measured a 0.003049 RMS ratio after 48→16 kHz conversion.
- **2026-08-04**: Plan 001 implemented. The ×0.005 gain row records one allowed no-transcript baseline for `LJ001-0002`; all other rows allow none. Extracted transcription handling from the microphone worker to clear the required lint gate without changing behavior.
- **2026-08-04**: Effort planned (8 plans) from the audio-capture audit.
  Capture fixes landed as `8bbf8294`, plans as `c52100db`. Next: 001.

## Considered and rejected

(So nobody re-plans these.)

- **Gain normalization / AGC for the ASR path**: sherpa's log-mel front-end
  is effectively level-invariant once samples are f32; the incident's real
  causes (U8 capture, absolute RMS gate) are fixed. 001's quiet rows verify
  the assumption; revisit only if they fail (their STOP condition).
- **Pre-roll standing mic stream** (fixes first-word clipping fully):
  reverses the deliberate idle-mic-release decision
  (`plans/gpui-rewrite-hardening/005`, DONE — privacy indicator + idle CPU).
  Reopen only with the open-latency data plan 005 starts logging.
- **Echo cancellation**: no far-end audio in a dictation app; system-level
  playback bleed is PipeWire's domain (`module-echo-cancel`), not Dictate's.
- **Committing degraded audio fixtures**: fixture rules forbid derived
  clips; degradations are generated in-memory (plan 001).
- **`audio.rs` >16-bit WAV precision loss** (downshift before normalize):
  fixture/CLI path only, near-zero impact.
- **Deduplicating the double `CapturedSignalMetrics::measure` call** on the
  success path: trivial cost, not worth a change.

## Deferred

- **Capture-side resampling row in the WER matrix**: needs a public resample
  seam that 002 deliberately doesn't add; noted in 001/002 maintenance notes.
- **Quiet-mic debug scenario** in `dictate-dev`: nice-to-have after 003;
  must embed the production component per repo rule.
- **Machine-readable `dictate devices --json`**: add when an agent actually
  needs it (008 maintenance notes).
- **Streaming/online ASR** (perceived-latency direction): out of this
  effort's scope entirely; belongs to the strategic roadmap.
