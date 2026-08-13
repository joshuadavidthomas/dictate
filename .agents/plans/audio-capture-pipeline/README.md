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

Plans 001–005 are build plans (001 first because it is the measurement
substrate and the incident lock); 006–007 are measurement spikes that produce
memos rather than production code; 008 is independent device-selection work.
The former combined Plan 009 is now four independent tracks: stereo spatial
processing, playback echo cancellation, anonymous speaker attribution, and
optional speaker identity. Plan 013 separately owns normal-use dev capture;
it is a diagnostic storage policy, not another analysis system.

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
| [006](006-vad-spike.md) | Spike: Silero VAD as the speech front-end | M–L | 001 | DONE |
| [007](007-denoiser-ab-spike.md) | Spike: denoiser A/B against raw ASR | S | 001 | BLOCKED — >2× cross-fixture variance; corpus too small |
| [008](008-input-device-selection.md) | Input device selection (setting + `devices` CLI) | M | — (after 002, 004, 005: file overlap) | DONE |
| [009](009-spatial-foreground-processing.md) | Compare native stereo foreground transforms | M | 001, 004 | DONE — no-go; fixed cancellation fails controlled acoustic calibration |
| [010](010-playback-echo-cancellation.md) | Validate client-owned playback echo cancellation | M–L | 001, 002, 004, 008 | BLOCKED — paused; prototype archived outside the usable build |
| [011](011-anonymous-speaker-attribution.md) | Land fail-open anonymous speaker attribution | M | 001 | BLOCKED — paused before the anonymous/identity split |
| [012](012-dev-speaker-identity-comparison.md) | Keep dev-only speaker identity comparison optional and separate | M | 011 | BLOCKED — paused; natural-use WAV capture continues |
| [013](013-dev-normal-use-capture.md) | Enable local normal-use WAV capture only in dev | S | 004 | IN REVIEW — clean build and capture smoke test pending |

Status values: TODO | IN REVIEW | IN PROGRESS | DONE | BLOCKED (one-line reason) |
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
- **006 + 007 constrain 009–012**: both spikes rejected generic frame-level
  speech removal. None of the later tracks may restore amplitude gates,
  Silero trimming, or GTCRN denoising under a new name.
- **001 → 009/010**: spatial and echo experiments use the existing language
  outcomes to protect quiet speech and report interferer insertion separately.
- **011 → 012**: identity may compare an anonymous speaker only after
  attribution exists independently. Anonymous analysis never needs a profile.
- **004 → 013**: Plan 004 provides safe optional WAV persistence; Plan 013
  alone owns enabling it on every dev-service start and its retain-until-delete
  policy. Ordinary dictation does not depend on spatial, echo, diarization,
  identity, or a corpus workflow.

## Reconciliation log

- **2026-08-05**: Audio exploration paused to restore a small daily-use checkpoint. The spatial, AEC, anonymous-speaker, and identity prototypes remain recoverable in local `jj` change `kwllkklu` but are absent from the usable build. Plans 010–012 are blocked without further implementation. Plan 013 remains because it saves completed dev dictations for later evidence without changing production capture.
- **2026-08-05**: Plan 009 closed with a no-go verdict. Consented fixed-geometry silence and continuous mono pink-noise recordings showed that the digital-microphone path attenuates steady broadband input over time: one 20-second capture dropped about 21 dB between halves, a repeat dropped about 20 dB, and an eight-second capture fell from -26.2 dBFS in its first four seconds to -50.4 dBFS in its last four. The frozen fit again found zero usable bins. The acoustic gate rejected fixed mid/side cancellation before ASR, and production averaging remains unchanged.
- **2026-08-05**: Plan 009 remained open after the constrained-cancellation follow-up. The probe added canonical content scores and bounded per-bin mid/side weights with a pre-ASR acoustic holdout. `tv-only.wav` produced zero bins that held 3 dB reduction from its first half to its second; its side RMS also changed from 0.00421 to 0.00109 between halves. That result prompted the controlled mono calibration above rather than weaker gates.
- **2026-08-05**: Plan 009's first sweep produced no production transform and the plan remains open. Its first reading overstated user harm because literal WER treated spelled numbers versus digits and `TV` versus `T V` as lost content. Raw side extraction did damage centered and quiet speech; integer delays did not prove TV attenuation against an independent reference. [`memo-spatial-foreground-results.md`](memo-spatial-foreground-results.md) now feeds the next in-plan phase: constrained mid/side cancellation and frequency-domain beamforming.
- **2026-08-05**: The [combined foreground/speaker design](memo-foreground-speaker-aware-design.md) was superseded by Plans 009–012. Spatial beamforming, playback AEC, anonymous attribution, and optional identity now have separate inputs, outputs, gates, and STOP conditions. Conversation sessions and remembered-voice storage have separate deferred ownership memos.
- **2026-08-05**: Plan 013 now owns natural dev dictation capture. It is enabled only in `dictate-dev.service` at `~/.local/state/dictate-dev/captures/`, saves local post-resample mono WAVs until manually removed, and does not alter production Dictate.
- **2026-08-05**: A consented same-speaker set measured Josh near/normal, near/quiet, and far/normal at −40.0, −47.3, and −51.3 dBFS over seconds 1–9. CAM++ similarity to the near-normal sample remained `0.924515` for quiet speech and `0.883380` at distance; TV-only scored `0.360202`. A user-plus-TV segment fell to `0.416075`, confirming that overlap still blocks identity thresholds and filtering.
- **2026-08-05**: Conversation ownership and retention are defined in [`memo-conversation-session-ownership.md`](memo-conversation-session-ownership.md). Conversation capture is explicit and local; sessions own retained audio, transcripts, anonymous labels, and deletion. Remembered voices are separate, require confirmation, store no raw excerpts, and never change default dictation.
- **2026-08-05**: The former combined Plan 009's timestamp and attribution gate cleared. The default Parakeet model returned complete monotonic token timelines without changing ordinary WAV transcripts. A dev-only JSON probe now reports anonymous speakers, unknown boundaries, true overlap, and optional profile similarity without dropping text. Profile audio excludes competing regions. Human cross-device thresholds and model distribution remain blocked on broader evidence and review.
- **2026-08-05**: The former combined Plan 009's first probe is recorded in [`memo-foreground-speaker-probe.md`](memo-foreground-speaker-probe.md). Native channels differed, the valid mixed-dialogue clip transcribed only the nearby user, CAM++ separated user-only from TV-only audio in this session, and full Pyannote diarization found plausible anonymous segments. The int8 segmentation model missed TV-only dialogue and is rejected pending parity evidence.
- **2026-08-05**: The original combined Plan 009 design was accepted, then later superseded by focused Plans 009–012 after the tracks proved independently useful.
- **2026-08-05**: Added the former combined Plan 009 design discussion after home testing showed that an independent television ten feet away could produce a valid transcript. The proposed default preserves all nearby speakers, uses native microphone channels only if they provide measured spatial evidence, adds anonymous diarization, and lets an explicit speaker label create an optional local remembered profile. Personal isolation remains opt-in so conversation capture can retain the user, a child, or other nearby speakers.
- **2026-08-05**: The in-process WebRTC AEC experiment is disabled by default behind `DICTATE_EXPERIMENTAL_ECHO_CANCELLATION`. Separate client-owned microphone and playback-monitor streams leave routing untouched, but their timing and quiet-speech behavior are not yet safe for production. AEC applies only to audio played by the laptop and cannot cancel an independent television.
- **2026-08-04**: Playback-aware echo cancellation began after controlled dock-microphone recordings proved that quiet YouTube bleed was intelligible beside the user's quiet speech. The rejected server-owned prototype was replaced with client-owned stream research so a crash cannot leave virtual devices or changed routes behind.
- **2026-08-04**: Plan 008 implemented. `dictate devices` lists PipeWire microphone sources on Linux with stable CPAL IDs, default markers, and configured markers; `input_device` stores an exact ID. The debug preview was updated for the clean-break capture signature.
- **2026-08-04**: Plan 007 stopped with a handback in [`memo-denoiser-ab.md`](memo-denoiser-ab.md). GTCRN effects varied by more than 2× and changed direction across fixtures; aggregate WER also worsened on clean, quiet, and 0 dB SNR rows.
- **2026-08-04**: Plan 006 completed with a no-go verdict in [`memo-vad-findings.md`](memo-vad-findings.md). Silero trimming regressed quiet-speech WER even at the first threshold that met the retention rule, and that threshold admitted generated noise as speech.
- **2026-08-04**: Plan 005 implemented. The overlay shows an inactive opening state until the current recording's microphone opens, then switches to the live waveform; successful opens log their latency.
- **2026-08-04**: Plan 004 implemented. Capture startup logs the selected device, and `DICTATE_CAPTURE_DIR` saves post-resample utterances as replayable 16-bit mono WAV files without replacing earlier captures.
- **2026-08-04**: Plan 003 implemented. The overlay spectrum now measures each band relative to an adaptive floor, keeps sustained structured signals visible while still adapting to stationary broadband and tonal noise, and gates silence flat.
- **2026-08-04**: Plan 002 implemented. The fallback capture path now uses sherpa-onnx's bandlimited streaming resampler, including an ordered final flush; the 12 kHz aliasing probe measured a 0.003049 RMS ratio after 48→16 kHz conversion.
- **2026-08-04**: Plan 001 implemented. The ×0.005 gain row records one allowed no-transcript baseline for `LJ001-0002`; all other rows allow none. Extracted transcription handling from the microphone worker to clear the required lint gate without changing behavior.
- **2026-08-04**: Effort planned (8 plans) from the audio-capture audit.
  Capture fixes landed as `8bbf8294`, plans as `c52100db`. Next: 001.

## Considered and rejected

(So nobody re-plans these.)

- **A corpus manager for normal dev dictation**: natural opt-in WAV capture is enough. Select files manually when a specific experiment needs them; do not add manifests, labels, or collection workflow without a demonstrated need.
- **Coupling beamforming to speaker identity**: spatial transforms use channel timing and level evidence. They must not require a profile, diarization, or transcript oracle.
- **Gain normalization / AGC for the ASR path**: sherpa's log-mel front-end
  is effectively level-invariant once samples are f32; the incident's real
  causes (U8 capture, absolute RMS gate) are fixed. 001's quiet rows verify
  the assumption; revisit only if they fail (their STOP condition).
- **Pre-roll standing mic stream** (fixes first-word clipping fully):
  reverses the deliberate idle-mic-release decision
  (`.agents/plans/gpui-rewrite-hardening/005`, DONE — privacy indicator + idle CPU).
  Reopen only with the open-latency data plan 005 starts logging.
- **PipeWire-managed echo-cancel devices**: the successful experiment required
  a virtual playback sink. A killed daemon could leave that sink as the saved
  system default. Dictate instead keeps both capture streams and AEC processing
  inside its own process, so a crash leaves no audio objects or routes behind.
- **Committing degraded audio fixtures**: fixture rules forbid derived
  clips; degradations are generated in-memory (plan 001).
- **`audio.rs` >16-bit WAV precision loss** (downshift before normalize):
  fixture/CLI path only, near-zero impact.
- **Deduplicating the double `CapturedSignalMetrics::measure` call** on the
  success path: trivial cost, not worth a change.

## Deferred

- **Conversation capture and review**: ownership and retention are defined in [`memo-conversation-session-ownership.md`](memo-conversation-session-ownership.md), but no current product need justifies sessions, labeling UI, or retained conversation audio.
- **Remembered voice storage**: ownership is defined independently in [`memo-remembered-voice-ownership.md`](memo-remembered-voice-ownership.md); Plan 012 remains a dev-only comparison and does not create profiles.
- **Personal isolation and target-speaker extraction**: no plan until ordinary use shows a recurring overlap problem and distributable models clear quiet-speech gates.
- **Capture-side resampling row in the WER matrix**: needs a public resample
  seam that 002 deliberately doesn't add; noted in 001/002 maintenance notes.
- **Quiet-mic debug scenario** in `dictate-dev`: nice-to-have after 003;
  must embed the production component per repo rule.
- **Machine-readable `dictate devices --json`**: add when an agent actually
  needs it (008 maintenance notes).
- **Streaming/online ASR** (perceived-latency direction): out of this
  effort's scope entirely; belongs to the strategic roadmap.
