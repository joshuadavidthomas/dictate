# Plan 012: Keep dev-only speaker identity comparison optional and separate

> **Executor instructions**: Follow this plan only after Plan 011 lands and the user explicitly resumes speaker-identity work. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback. Do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from e63dfe42 --to @ -- crates/dictate-speech/src/speaker.rs crates/dictate-speech/src/speaker_models.rs crates/dictate-speech/src/lib.rs crates/dictate-dev/src/speaker_probe.rs crates/dictate-dev/src/lib.rs crates/dictate/src/cli.rs .agents/plans/audio-capture-pipeline/memo-remembered-voice-ownership.md`
> Identity comparison exists experimentally inside archived local change `kwllkklu`. Plan 011 is expected to separate anonymous attribution before this plan runs. Stop if that separation has not landed.

## Status

- **Status**: BLOCKED — paused; natural-use WAV capture continues without identity analysis
- **Effort**: M
- **Risk**: HIGH
- **Depends on**: Plan 011
- **Planned at**: working-copy revision `e63dfe42`, 2026-08-05

## Why this matters

Speaker identity asks whether an anonymous speaker resembles an explicitly chosen local voice profile. It is distinct from diarization, spatial location, and transcript inclusion. A useful experiment must preserve uncertainty, bind embeddings to verified model bytes, and avoid turning normal dictation into an enrollment or filtering workflow.

This plan provides a dev-only identity comparison seam. Persistent remembered voices have a separate deferred ownership memo; this plan does not create them or choose a production threshold.

## Current state

- The combined `SpeakerAnalyzer` can currently compare each anonymous speaker's exclusive audio with a supplied profile WAV.
- Profile and target extraction trim 200 ms inside diarization boundaries, remove competing regions and boundary margins, round inward to sample frames, and merge overlapping same-speaker ranges.
- `SpeakerEmbedding` is private, carries the verified CAM++ digest, checks vector dimensions and finite stable norms, and rejects incompatible model identities.
- Current isolated samples scored `0.924515` for quiet speech and `0.883380` for far speech against the near-normal sample. TV-only scored `0.360202`.
- In a mixed user-plus-TV segment, the apparent user score fell to `0.416075`. Simultaneous speech contaminates identity evidence and forbids a deletion threshold.
- Natural dev dictations are saved as local 16 kHz mono WAVs under `~/.local/state/dictate-dev/captures/`. They are optional source material, not a corpus product or automatic enrollment stream.

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

- A dedicated identity module under `crates/dictate-speech/src/` that compares verified embeddings from exclusive speech interiors.
- `crates/dictate-speech/src/speaker_models.rs` — typed model identity and byte verification.
- `crates/dictate-speech/src/lib.rs` — a narrow dev-facing identity result.
- A separate dev-only identity probe under `crates/dictate-dev/src/`.
- `crates/dictate/src/cli.rs` — dev-only command arguments.
- Identity findings memo and effort README.

**Out of scope**:

- Anonymous diarization and token attribution — Plan 011.
- Spatial location — Plan 009.
- Playback AEC — Plan 010.
- Persistent profile storage, enrollment UI, or automatic learning — deferred in `memo-remembered-voice-ownership.md`.
- Conversation sessions and labels.
- Any inclusion, rejection, suppression, or personal-isolation policy.
- A corpus manager, manifest, benchmark service, or mandatory recording routine.
- Production thresholds selected from the existing samples.

## Steps

### Step 1: Define the identity boundary

Create an identity analyzer whose inputs are:

- verified identity model files;
- one explicit profile WAV or already-validated embedding set;
- anonymous speaker-exclusive audio supplied by Plan 011 or a dev probe.

Its output reports model identity, candidate speaker, finite optional similarity, and a typed failure. It must not return `is_match`, `include`, `reject`, or another policy decision.

Dependency-native embeddings remain private. Similarity requires identical verified model digests and dimensions.

**Verify**: focused tests reject digest mismatch, shape mismatch, empty vectors, non-finite values, unstable norms, and non-finite cosine output.

### Step 2: Keep profile evidence exclusive

A profile WAV must first pass anonymous diarization and contain exactly one anonymous speaker. Embed only exclusive segment interiors after removing overlap, competitors, and boundary margins. Merge same-speaker ranges so samples are never duplicated. Fail if too little clean speech remains.

Do not embed whole files, overlapping regions, silence padded around speech, or mixed user-plus-TV segments.

**Verify**: tests cover inward rounding, competing regions, boundary slop, overlapping same-speaker ranges, short audio, and multi-speaker profile rejection.

### Step 3: Add a separate headless identity probe

Add a command distinct from anonymous `speaker-probe`. It takes target WAV, profile WAV, segmentation model, and embedding model. Emit JSON containing the raw target transcript, anonymous attribution status, and independent identity status. Profile failure must not erase complete anonymous attribution or raw text.

No command may scan the dev capture directory, infer labels from filenames, or remember a voice automatically.

**Verify**: valid, missing-profile, bad-model, overlap-contaminated, and multi-speaker-profile cases emit parseable JSON with meaningful exit codes.

### Step 4: Use natural captures only when a question requires them

If identity work resumes, manually select a few consented natural dev WAVs that answer a concrete question such as cross-session stability. Keep results in a local memo. Do not build collection, labeling, or retention machinery.

Exact threshold selection remains blocked while same-speaker overlap scores collide with different-speaker scores or while device/model distribution gates remain open.

**Verify**: the memo states the question, selected files, exact model digest, extraction rule, scores, and a no-threshold verdict unless a later accepted plan defines broader evidence.

### Step 5: Stop before product behavior

Do not add persistent profiles, settings, UI, filtering, or daemon policy. Those require a separate accepted product plan based on an observed need. The existence of identity code must not change ordinary dictation.

**Verify**: `just fmt && just check && just test && just test-integration && just lint && just hawk && just build --release` → all pass.

## Test plan

- Model-byte identity and vector safety checks.
- Exclusive profile extraction and same-speaker range merging.
- Multi-speaker and overlap-contaminated profile rejection.
- Profile failure preserves anonymous attribution and raw text.
- The identity command remains dev-only.
- Ordinary dictation and WAV transcription do not initialize identity models.

## Done criteria

- [ ] Identity is a separate dev-only module and command.
- [ ] It reports similarity and uncertainty, never an inclusion policy.
- [ ] Profile evidence comes only from exclusive single-speaker interiors.
- [ ] No automatic corpus, enrollment, storage, or filtering behavior exists.
- [ ] Full verification passes.

## STOP conditions

Stop and write a handback if:

- Plan 011 anonymous attribution is not independent and complete.
- A use case requires identity to choose a spatial transform.
- Overlapping speech must be treated as clean identity evidence.
- A threshold would reject known same-speaker samples or accept known different-speaker samples.
- Model redistribution rights, digest, provenance, release CPU, or memory remain unclear for a proposed production path.
- The work creates persistent voice data without a separate explicit user decision and deletion contract.
- A verification command fails twice after a reasonable fix.

## Maintenance notes

Similarity is evidence, not policy. If remembered voices become a real product need, start a new plan from `memo-remembered-voice-ownership.md` and require visible create, inspect, rename, and delete actions. Do not extend this dev probe into a hidden profile store.
