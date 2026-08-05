# Plan 011: Land fail-open anonymous speaker attribution as a dev-only analysis path

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback. Do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from e63dfe42 --to @ -- crates/dictate-speech/src/transcription.rs crates/dictate-speech/src/speaker.rs crates/dictate-speech/src/speaker_models.rs crates/dictate-speech/src/lib.rs crates/dictate-speech/tests/integration.rs crates/dictate-dev/src/speaker_probe.rs crates/dictate-dev/src/lib.rs crates/dictate/src/cli.rs`
> Anonymous attribution and profile comparison share code in archived local change `kwllkklu`; they are absent from the usable build. Reconcile drift and stop if the typed transcript or report shapes have changed.

## Status

- **Status**: BLOCKED — paused before separating anonymous attribution from identity
- **Effort**: M
- **Risk**: MEDIUM
- **Depends on**: Plan 001
- **Planned at**: working-copy revision `e63dfe42`, 2026-08-05

## Why this matters

Anonymous diarization can describe when `Speaker 1` and `Speaker 2` spoke without deciding who they are or deleting any text. It supplies evidence for later conversation or identity work while leaving ordinary dictation untouched. The current working copy proves timed ASR and fail-open attribution, but optional profile comparison is mixed into the same analysis type and command.

This plan would land anonymous attribution alone if work resumes. Remembered identity belongs to Plan 012.

## Current state

- `crates/dictate-speech/src/transcription.rs` preserves `RecognizedTranscript` and `RecognizedToken` timing metadata while retaining raw-text parity.
- `crates/dictate-speech/src/speaker.rs::SpeakerAnalyzer` runs full Pyannote diarization, token attribution, exclusive segment extraction, CAM++ embedding, and optional profile comparison in one module.
- `SpeakerAttribution` distinguishes `Speaker`, `Unknown`, `Ambiguous`, and true common-interval `Overlap`.
- Invalid or incomplete token intervals disable attribution without dropping raw text.
- `crates/dictate-dev/src/speaker_probe.rs` emits typed JSON and exits nonzero after printing retained raw text when analysis fails.
- `crates/dictate-speech/src/speaker_models.rs` verifies exact Pyannote and CAM++ bytes before this path can initialize.
- Full Pyannote is required for the observed TV-only clip; the int8 model missed that dialogue and is rejected.

The boundary convention is already established: sherpa-native cluster IDs and timestamps become owned session speaker and transcript types inside `dictate-speech`.

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

- `crates/dictate-speech/src/transcription.rs` — timed recognition metadata with raw-text parity.
- `crates/dictate-speech/src/speaker.rs` — anonymous timeline, attribution, and fail-open report.
- `crates/dictate-speech/src/speaker_models.rs` — verified model family needed by diarization.
- `crates/dictate-speech/src/lib.rs` — narrow exported analysis interface.
- `crates/dictate-speech/tests/integration.rs` — model-backed timestamp characterization.
- `crates/dictate-dev/src/speaker_probe.rs`, `crates/dictate-dev/src/lib.rs`, and `crates/dictate/src/cli.rs` — anonymous dev probe.
- Speaker-probe findings memo and effort README.

**Out of scope**:

- Profile WAV arguments, identity similarity, remembered labels, or inclusion thresholds — Plan 012.
- Conversation storage or review UI — deferred in `memo-conversation-session-ownership.md`.
- Filtering audio or transcript text.
- Ordinary `dictate transcribe <wav>` behavior.
- Model downloads through the ASR catalog.
- Production daemon invocation of diarization.

## Steps

### Step 1: Keep timed recognition independent from speaker analysis

Land `RecognizedTranscript` and `RecognizedToken` as speech-owned decode results. Valid timed tokens must reconstruct the raw transcript, have monotonic in-source starts, and expose positive durations when they can be safely normalized. Malformed timing removes attribution metadata but preserves raw text.

Trailing zero-duration token groups may use the remaining source duration. Reject non-finite, negative, reversed, and out-of-source intervals.

**Verify**: `cargo test -p dictate-speech transcription` and `just test-integration` → raw-text parity and timing tests pass.

### Step 2: Separate anonymous analysis from identity comparison

Refactor the public anonymous analyzer so its input is only an utterance and recognized transcript. Its output contains raw text, anonymous speaker segments, and attributed tokens or an attribution failure. It must not accept profile audio, compute identity similarity, expose remembered labels, or carry a profile status field.

Remove profile comparison, profile report fields, identity-only embedding comparison, and identity-only extraction from the Plan 011 landed change. Preserve the experimental findings in `memo-foreground-speaker-probe.md`; Plan 012 may reintroduce identity code later under its own boundary. Do not implement any part of blocked Plan 012 while landing this plan.

**Verify**: `cargo test -p dictate-speech speaker` → anonymous analysis tests pass and profile-specific tests no longer belong to this module's public report.

### Step 3: Preserve exact uncertainty semantics

Keep these rules:

- `Speaker`: one candidate covers the usable token interval.
- `Overlap`: every reported candidate shares one common positive interval.
- `Ambiguous`: sequential boundaries, competing candidates without common simultaneity, and overlap chains.
- `Unknown`: no candidate covers the interval.

Every token remains in output. Missing or invalid intervals fail the attribution report while retaining raw text.

**Verify**: focused tests cover malformed timelines, zero and out-of-source intervals, sequential boundaries, true overlap, and overlap chains.

### Step 4: Make `speaker-probe` anonymous and machine-readable

The base `speaker-probe` command takes a WAV plus verified segmentation and embedding model files, runs ASR and anonymous attribution, and emits one JSON document. Remove profile arguments from this command. A model, diarization, or timing failure prints JSON containing the raw transcript before returning nonzero with `speaker analysis failed; inspect the JSON report`.

A separate identity command may be added only under Plan 012.

**Verify**: good and invalid model-path invocations both produce parseable JSON; the latter retains raw text and exits nonzero.

### Step 5: Confirm production isolation

Speaker models and APIs remain in `dictate-speech`, but only the debug-profile `dev-tools` path invokes them. Ordinary WAV transcription and microphone dictation must produce the same text and must not initialize speaker models.

**Verify**: `just fmt && just check && just test && just test-integration && just lint && just hawk && just build --release` → all pass.

## Test plan

- Timed tokens reconstruct raw text and remain within source bounds.
- Trailing zero-duration groups normalize without accepting malformed intervals.
- Anonymous IDs are stable only inside one report.
- Sequential boundaries and overlap chains are ambiguous.
- True common simultaneity is overlap.
- Missing timestamps preserve raw text and fail attribution.
- Bad model paths preserve raw-text JSON and return nonzero.
- Production transcription never calls speaker analysis.

## Done criteria

- [ ] Timed recognition lands without changing ordinary transcript text.
- [ ] Anonymous attribution has no profile or identity contract.
- [ ] Every failure retains raw text.
- [ ] The dev probe emits typed JSON and meaningful exit codes.
- [ ] Full verification passes.
- [ ] No production policy filters or labels speech.

## STOP conditions

Stop and write a handback if:

- Reliable token intervals cannot be obtained without changing raw transcripts.
- Quiet or short speech disappears from the raw result.
- Anonymous attribution requires an identity threshold or profile.
- A model failure can prevent ordinary transcription.
- The analyzer must persist audio or labels.
- The model asset enters the ordinary ASR catalog.
- A verification command fails twice after a reasonable fix.

## Maintenance notes

Anonymous cluster numbers are session-local facts, not identities. Keep this module useful without Plan 012, conversation sessions, or any filtering policy. Model redistribution remains a release concern even when analysis is dev-only.
