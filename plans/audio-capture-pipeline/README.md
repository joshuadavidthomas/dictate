# Audio capture pipeline

Fixes and investigations from the 2026-08-04 `/improve` audit of the audio
capture path, prompted by a real incident: a normal headset's speech (RMS
0.002141, −53 dBFS) was captured correctly and then discarded by a fixed
amplitude gate, with the overlay meter showing near-silence throughout. The
audit's thesis: **level-invariance must be a property of the whole pipeline**
— capture format, gating, metering, and evaluation — not a patch at one
point, and every "should we add pipeline stage X" question (VAD, denoising)
gets decided by WER measurement, not by imitating meeting software.

Planned at revision `nootnkmorwsk` (git `cc3223f80bfb`) — note this is the
**working-copy snapshot** containing the (then-uncommitted) capture-format
preference fix and RMS-gate removal from the incident debugging session;
those changes are vetted and assumed present. **Precondition: describe/commit
that working copy (`jj commit`) before executing any plan here.**

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
| [001](001-degradation-eval-matrix.md) | Extend the WER harness with a degradation matrix | M | — | TODO |
| [002](002-bandlimited-resampler.md) | Replace the fallback resampler with sherpa's bandlimited one | S–M | — | TODO |
| [003](003-adaptive-overlay-meter.md) | Make the overlay meter adapt to signal level | M | — | TODO |
| [004](004-capture-diagnostics.md) | Capture diagnostics: device name + persistable audio | S | — (after 002: file overlap) | TODO |
| [005](005-honest-recording-overlay.md) | Show "Recording" only when the mic is live | M | — (after 004: file overlap) | TODO |
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

- **2026-08-04**: Effort planned (8 plans) from the audio-capture audit.
  Next: commit the working copy, then 001.

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
