---
type: superseded-design-discussion
repo: dictate
branch: ""
sha: f0af5d224199
status: superseded
superseded_by: 009-spatial-foreground-processing.md, 010-playback-echo-cancellation.md, 011-anonymous-speaker-attribution.md, 012-dev-speaker-identity-comparison.md
source_research: repository inspection, controlled playback tests, Claude architecture review, sherpa-onnx 1.13.2 API and model research
---

# Superseded design discussion: Foreground and speaker-aware capture

This document preserves the original combined design. It bundled independent playback, spatial, anonymous-speaker, identity, and conversation concerns. Plans 009–012 now own the technical tracks separately. Plan 013 owns normal dev capture. Conversation sessions and remembered-voice storage remain deferred in separate ownership memos.

## Summary of change request

Dictate must handle three kinds of unwanted speech without confusing them:

1. Audio played by the same computer can be canceled from a PipeWire playback reference.
2. A television or person elsewhere in the room has no playback reference. Spatial processing may lower speech outside the nearby conversation area.
3. When location is insufficient, speaker diarization and remembered speaker labels can express which voices the user wants.

The default behavior must preserve nearby speakers. A user may dictate alone, dictate beside another person, or review a conversation with several speakers. Remembering a voice is optional and begins naturally when the user labels an anonymous speaker. Dictate must not silently decide that only one enrolled person is allowed to speak.

## Review status

- **Status:** Superseded after acceptance
- **Accepted:** 2026-08-05 after review of multi-speaker labeling, optional remembered voices, and default foreground behavior.
- **Superseded:** 2026-08-05 by focused Plans 009–012. This memo is historical context, not an execution plan.

## What better means

- A nearby quiet speaker remains transcribable at the incident level already protected by the degradation matrix.
- Dictate lowers word insertions from a distant television when microphone channels contain useful spatial evidence.
- Dictate can render anonymous `Speaker 1` / `Speaker 2` transcript segments.
- A user can label a speaker and optionally remember that voice locally, without a separate scripted enrollment flow.
- A conversation can retain several nearby speakers, including speakers without stored profiles.
- Personal isolation is explicit. It never becomes the default for ordinary dictation or conversation capture.
- Uncertain spatial, diarization, or identity results preserve the raw capture and transcript.
- `dictate transcribe <wav>` keeps its current behavior unless a future explicit speaker-analysis option is supplied.
- No stage creates PipeWire nodes, changes defaults, moves streams, or depends on process cleanup to restore audio routing.

A regression is any quiet-user deletion, nearby second-speaker deletion, new default profile requirement, hidden persistence of voice data, or ordinary WAV transcription change.

## Standards and design pressure

The design review applied the coding-standards skill's domain granularity rule: playback echo, foreground location, anonymous speaker clusters, remembered speaker identity, and transcript inclusion policy cause different behavior and must remain separate concepts.

It also applied the skill's boundary rule at the CPAL and sherpa-onnx edges. CPAL's negotiated channel shape must become an owned capture representation before core decisions. Sherpa's numeric cluster IDs, timestamps, and vectors must become session speakers, transcript segments, and stored profiles rather than leaking dependency vocabulary through the app.

The skill's verification rule requires behavior evidence. Compiler success cannot prove TV rejection or quiet-speech retention. The effort needs mixed-speaker fixtures and measured insertion/deletion rates before filtering can ship.

The final speech-analysis module should be deep: callers provide captured audio and an explicit inclusion policy; the module owns model sequencing, timestamp alignment, confidence, fallback, and diagnostics. Do not make daemon or UI callers reproduce those rules.

## Reconnaissance summary

- `crates/dictate-speech/src/mic.rs::build_input_stream` averages every input frame across channels inside the CPAL callback. Directional timing and level evidence is gone before worker processing.
- PipeWire reports the current laptop digital microphone as a hardware ALSA source at `hw:acp`, 48 kHz, two channels, with `FL` and `FR` positions. This is evidence for a native two-channel path, but only a raw stereo comparison can prove the channels carry distinct capsule signals.
- `crates/dictate-speech/src/transcription.rs::Recognizer::decode` currently keeps only `OfflineRecognizerResult::text`. sherpa-onnx 1.13.2 also exposes tokens and optional timestamps. Its recognizer configuration can enable token and segment timestamps.
- sherpa-onnx 1.13.2 exposes Rust interfaces for `SpeakerEmbeddingExtractor`, `SpeakerEmbeddingManager`, and `OfflineSpeakerDiarization`.
- The recommended first embedding candidate is the English CAM++ VoxCeleb ONNX model, approximately 28.2 MiB. The 3D-Speaker model lineage is Apache-2.0; distribution still needs recorded provenance, checksum, and bundled license text.
- sherpa's diarization path combines a Pyannote segmentation model, speaker embeddings, and clustering. Cluster numbers are anonymous and unstable across recordings until compared with remembered profiles.
- Speaker verification can reject TV-only audio or label alternating speakers. It cannot recover a user's waveform when the user and television speak simultaneously.
- Target-speaker extraction can address overlapping voices but sherpa-onnx 1.13.2 does not expose such a model. Current candidates introduce model, licensing, runtime, and quiet-speech risks.
- Plan 006 rejected Silero VAD because thresholds that retained quiet speech still worsened WER. Plan 007 rejected GTCRN because quality varied by fixture and aggregate WER worsened. Neither should be renamed or retuned as a speaker solution.
- The WebRTC AEC experiment is now disabled by default behind `DICTATE_EXPERIMENTAL_ECHO_CANCELLATION`. It addresses laptop playback only. It cannot cancel an independent television.

## Current state

The microphone callback turns negotiated input into 16 kHz mono audio, which the daemon accumulates into one `CapturedUtterance`. The offline recognizer returns one text string. Dictate has no channel evidence, speaker timeline, persistent speaker label, or transcript review surface.

This means all intelligible speech is valid ASR input. A distant television can produce a plausible transcript. Dictate cannot distinguish that dialogue from a nearby child without spatial or identity evidence.

The production interaction is insertion-first: stop recording, transcribe, format, and insert into the focused app. Dictate does not retain a transcript session where a user could relabel speakers. That makes speaker review a product surface decision, not a small transcription flag.

## Desired end state

Dictate supports three explicit behaviors:

### Foreground dictation

The normal path preserves all speech in a conservative nearby pickup area. It can use native microphone channels to lower off-axis or diffuse room speech. It does not require profiles and fails open when spatial confidence is weak.

### Conversation review

A separate capture/review flow presents timestamped transcript segments as `Speaker 1`, `Speaker 2`, and so on. The user can rename speakers. A label can remain session-only or, with an explicit “Remember this voice on this device” choice, create or update a local profile from high-confidence non-overlapping segments.

### Personal isolation

An explicit personal mode may retain one or more selected remembered speakers and reject other voices. It is never inferred from the mere existence of a profile. Ambiguous identity or overlap returns the unfiltered result.

Playback-aware AEC remains an independent preprocessing stage for audio emitted by the same computer.

## What we are not doing

- Treating external television speech as echo.
- Restoring an amplitude gate, stronger generic VAD, or generic mono denoiser.
- Requiring a voice profile before Dictate works.
- Filtering every unknown speaker by default.
- Claiming that two negotiated CPAL channels prove a usable microphone array.
- Shipping target-speaker extraction before mixed-speech evidence exists.
- Building a generic meeting-recording product inside the small recording overlay.
- Persisting raw enrollment audio by default.
- Changing persistent system audio routing.

## Proposed end-state architecture

```text
CPAL / PipeWire input
        |
        v
native capture adapter
(sample rate, channel count, interleaved samples, timing)
        |
        +--> raw fallback held for this utterance
        |
        v
optional laptop-playback AEC
        |
        v
conservative spatial foreground processor
        |
        v
16 kHz mono CapturedUtterance
        |
        +--> offline ASR tokens + timestamps
        |
        +--> diarization segments + anonymous session speakers
        |
        +--> optional profile matching
        |
        v
attributed transcript
(words, time ranges, session speaker, confidence)
        |
        +--> immediate insertion policy
        +--> conversation review and labeling
        +--> explicit personal-isolation policy
```

All audio and speaker analysis belongs to `dictate-speech`. CPAL and sherpa-native values stay inside adapters there. `dictate` owns settings, orchestration, and local profile storage policy. `dictate-ui` owns production review views. `dictate-dev` owns diagnostic recordings, channel plots, model comparisons, and reproducible scenarios.

Do not add a shared types crate. Export only the result and command types required by these owning crates.

## Design questions

### Where should users label speakers?

- **Option A: interrupt every dictation when several speakers are detected.** This exposes labels early but damages the fast insert workflow and makes model mistakes block ordinary use.
- **Option B: add conversation capture/review as a separate flow.** Ordinary dictation remains immediate. Conversation capture retains the transcript and audio long enough to relabel segments and optionally remember voices.
- **Option C: retain a hidden history and let users review the last dictation later.** This avoids interruption but introduces audio retention and deletion policy before the product has a visible session model.
- **Recommendation:** Option B. It gives speaker labeling a truthful home and avoids hidden retention. A later “review last dictation” action can be designed only after retention semantics are explicit.

### How should foreground location be selected?

- **Option A: fixed broadside beam toward the normal user position.** Small interface and predictable behavior, but it depends on laptop geometry and can weaken a nearby speaker sitting to one side.
- **Option B: adapt to the strongest nearby active direction.** Better for moving speakers, but a loud television can become the chosen direction.
- **Option C: broad foreground region with a suppression floor.** Preserve several nearby directions and attenuate only strong off-axis evidence; fail open when evidence is weak.
- **Recommendation:** Start with Option C if the stereo diagnostic proves useful direction evidence. Evaluate the result against both one-user and user-plus-child fixtures. Do not choose an algorithm before the channel measurements.

### What should happen when identity is uncertain?

- **Option A: reject unknown or low-score segments.** Removes more television speech but repeats the quiet-speech failure in a new form.
- **Option B: keep uncertain segments and mark them unknown.** May retain unwanted television speech but does not delete a quiet nearby speaker.
- **Recommendation:** Option B. Personal isolation may surface uncertainty to the user, but its fallback remains the unfiltered transcript.

### How should overlapping speech be handled?

- **Option A: discard overlap.** Guaranteed user-word loss.
- **Option B: keep overlap unchanged and attribute it as ambiguous.** Safe, but television words can remain.
- **Option C: run target-speaker extraction.** Potentially recovers selected voices but adds an unproven waveform-changing model.
- **Recommendation:** Ship Option B first. Evaluate Option C later behind an experimental seam that always retains and can decode the raw utterance.

## Resolved design questions

### Is speaker memory the default filter?

No. Default foreground dictation preserves nearby speakers. A remembered profile affects output only under an explicit policy.

### How does a profile begin?

Labeling a diarized speaker is the primary enrollment interaction. The user chooses whether the label is session-only or remembered on the device. A separate scripted enrollment command may exist as a headless test and recovery path, not as the required product flow.

### What is stored?

A remembered speaker stores a label, several model embeddings or their model-supported aggregate, model identity and checksum, and format version. Raw audio is not retained by default. Users can inspect and delete remembered voices.

### Are profiles single-person only?

No. Several profiles may exist, and an explicit inclusion policy may select one, several, all known speakers, or all foreground speakers.

### Does diarization solve overlap?

No. It identifies time regions and anonymous speakers. It does not produce a clean waveform for simultaneous talkers.

### Does AEC solve external TV dialogue?

No. AEC runs only when a matching laptop playback reference exists. External TV dialogue proceeds to spatial and speaker analysis as room speech.

## Patterns to follow

### Preserve raw input on an uncertain processing stage

`crates/dictate-speech/src/mic.rs::EchoPipeline::emit_pending_raw` establishes the intended failure direction: when processing loses its reference or fails, emit raw microphone audio instead of silence. Spatial and speaker-aware stages must follow the same rule.

### Keep model families typed

`plans/audio-capture-pipeline/memo-vad-findings.md` records why a VAD asset must not be disguised as an ASR `ModelCatalogEntry`. Speaker embedding, diarization, and later extraction models need typed descriptors while sharing only download mechanics that truly match.

### Verify language outcomes rather than waveform aesthetics

`crates/dictate-speech/tests/integration.rs` and Plan 001 provide the WER harness and quiet-gain baselines. Extend that harness with intended-speaker references and interferer transcripts rather than approving a stage from RMS or listening alone.

## Standing policy and evaluation recommendations

Add the following standing rules to this effort and later executor plans:

- Every speech-removal stage is evaluated on quiet target speech, nearby multiple speakers, distant competing speech, alternating speakers, and overlap.
- Report target-speaker WER and interferer word-insertion rate separately. An aggregate average must not hide target deletion.
- Preserve per-fixture results and stop when effects change direction or vary by more than the predeclared bound.
- Identity confidence never substitutes for an amplitude threshold.
- Stored speaker profiles are local, visible, deletable, versioned by model, and created only by an explicit remember action.
- Default dictation preserves unknown nearby speakers.
- Every interactive speaker workflow has a headless equivalent with machine-readable output and meaningful exit codes.

## Evidence gates before implementation slices

The structure outline should route work through these gates:

1. **Native-channel diagnostic:** capture channels separately and measure correlation, lag, level, and direction for TV-only, user-only, and mixed speech. STOP spatial work if the channels are duplicates or direction is unstable.
2. **Mixed-speech evaluation corpus:** add deterministic or consented local fixtures with target and interferer transcripts. Define target WER, interferer insertion, attribution error, nearby-speaker retention, and latency budgets before selecting thresholds.
3. **Timestamp characterization:** enable sherpa timestamps in a headless path and prove tokens align with diarization segments for the current default model.
4. **Anonymous diarization:** produce speaker timelines without filtering audio or text. STOP if quiet or short turns cannot be segmented without unacceptable misses.
5. **Label-to-profile spike:** use high-confidence, non-overlapping labeled segments to build local CAM++ profiles. Measure same-speaker and different-speaker score distributions across devices before choosing a threshold.
6. **Conversation review:** add labels and explicit remember/delete actions only after session ownership and audio retention are designed.
7. **Explicit inclusion policies:** filter only after the unfiltered attributed transcript exists and fallback behavior is tested.
8. **Overlap extraction spike:** proceed only if overlap remains a measured user problem and a distributable model clears quiet-speech, runtime, and license gates.

## STOP conditions

Stop and return to design review if:

- The native laptop channels are duplicates or do not provide repeatable source-direction evidence.
- The default ASR model does not provide timestamps reliable enough to align words with speaker segments.
- Diarization misses quiet user turns at a rate that could silently delete content.
- A speaker-model threshold cannot separate same-speaker and different-speaker samples across home and work microphones without false rejection.
- A model asset lacks clear redistribution rights, checksum, provenance, or feasible CPU behavior.
- Speaker labeling requires hidden audio retention or blocks ordinary insertion.
- An implementation proposal merges foreground, identity, and inclusion policy into one confidence score.
- Target-speaker extraction requires a second inference runtime without a packaging and symbol-conflict audit.

## Rejected approaches

- **One mandatory Josh profile:** fails the nearby multi-speaker use case and makes identity an undeclared default policy.
- **Call diarization “enrollment” and require a setup recording:** adds ceremony when natural labeled speech already provides samples.
- **Silently learn every labeled speaker forever:** hides persistent derived voice data. Remembering must be explicit.
- **Suppress all unknown speakers:** removes a child, guest, or collaborator by default.
- **Use VAD or loudness as identity:** television dialogue is speech, and quiet target speech already disproved amplitude gates.
- **Use generic source separation without labels:** separated outputs still need a rule for which voice to keep.

## Stop gate

Stop here for design review. Confirm the recommended product behaviors, conversation-review surface, and fail-open rules before writing the structure outline.
