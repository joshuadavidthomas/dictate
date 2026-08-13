# Plan 013: Enable local normal-use WAV capture only in the development service

> **Executor instructions**: Follow this plan step by step. Run every verification command and confirm the expected result before moving on. If anything in "STOP conditions" occurs, stop and write a handback. Do not improvise. When done, update this plan's status row in the effort README.
>
> **Drift check (run first)**: `jj diff --from e63dfe42 --to @ -- systemd/dictate-dev.service systemd/dictate.service crates/dictate/src/daemon.rs crates/dictate-speech/src/audio.rs`
> The development service enables capture in the current usable checkpoint. Stop if production service policy or `CapturePersistence` semantics have changed.

## Status

- **Status**: IN REVIEW — installed and healthy; one natural-use capture smoke test remains
- **Effort**: S
- **Risk**: MEDIUM
- **Depends on**: Plan 004
- **Planned at**: working-copy revision `e63dfe42`, 2026-08-05

## Why this matters

Natural dev dictations provide realistic local WAVs for later debugging without staged recording sessions. This is a development diagnostic policy, not a corpus product, analysis system, or production feature. Its storage location and retain-until-delete behavior must be explicit because the service enables it on every dev daemon start.

## Current state

- Plan 004 added `CapturePersistence` in `crates/dictate/src/daemon.rs`. When `DICTATE_CAPTURE_DIR` is set, it creates the directory, saves each completed post-resample utterance as a unique 16-bit 16 kHz mono WAV, logs the path, and warns without interrupting transcription on failure.
- `systemd/dictate-dev.service` now sets `DICTATE_CAPTURE_DIR=%h/.local/state/dictate-dev/captures`.
- `systemd/dictate.service` does not set that variable.
- Captures contain only the microphone interval opened by a normal dictation action. They are not continuous ambient recordings.
- Files remain local until manually removed. There is no automatic expiry, upload, transcript sidecar, label, or corpus index.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format | `just fmt` | exit 0 |
| Check | `just check` | exit 0 |
| Unit tests | `just test` | all pass |
| Lint | `just lint` | exit 0 |
| Install dev | `just install-dev` | service installed and restarted |
| Service state | `systemctl --user is-active dictate-dev.service` | `active` |

## Scope

**In scope**:

- `systemd/dictate-dev.service` — opt-in environment for the installed dev daemon.
- `crates/dictate/src/daemon.rs` — only if tests or disclosure need correction.
- `crates/dictate-speech/src/audio.rs` — only if WAV persistence invariants need correction.
- This plan and the effort README.

**Out of scope**:

- `systemd/dictate.service` changes.
- Native stereo capture, spatial analysis, speaker attribution, or identity.
- Transcript sidecars, labels, manifests, search, import, export, or a corpus manager.
- Automatic enrollment or analysis of saved files.
- Hidden retention outside the declared directory.

## Steps

### Step 1: Lock the dev-only service boundary

Keep `DICTATE_CAPTURE_DIR` only in `systemd/dictate-dev.service`. The production service must remain free of capture persistence settings. Use the explicit local state path `~/.local/state/dictate-dev/captures/` after systemd specifier expansion.

**Verify**: `systemctl --user show dictate-dev.service --property=Environment --value` contains the expanded dev capture path after installation, while the installed production service environment does not contain `DICTATE_CAPTURE_DIR`.

### Step 2: Preserve capture and failure semantics

A file is written only after a completed dictation utterance exists. Each file must use a new name and never overwrite an earlier capture. Directory creation or write failure warns in the daemon log, leaves existing files untouched, and does not block transcription or insertion.

The daemon startup log must state the path and that saved audio remains until removed. This plan deliberately accepts retain-until-delete storage because the user explicitly enabled the dev diagnostic. Disk-full and permission errors fail open to normal transcription.

**Verify**: focused tests cover unique paths, no overwrite, WAV shape, and persistence failure without transcription mutation.

### Step 3: Keep the saved artifact intentionally plain

Save only the existing post-resample mono utterance. Do not add identity assumptions, transcript metadata, labels, indexes, or automatic analysis. A later experiment may select individual files manually.

Native stereo companion capture requires a separate plan after Plan 009 shows spatial value.

**Verify**: one consented normal dev dictation creates one replayable 16 kHz mono WAV in the declared directory and transcribes normally.

### Step 4: Record manual retention behavior

Document the removal path in the effort README: deleting WAVs from `~/.local/state/dictate-dev/captures/` is the retention control. Do not silently delete diagnostic files or invent automatic expiry in this plan.

**Verify**: restart the service and confirm it does not overwrite existing captures; manually removed files remain absent.

### Step 5: Run normal gates

**Verify**: `just fmt && just check && just test && just lint && just install-dev` followed by `systemctl --user is-active dictate-dev.service` → all pass and service is `active`.

## Test plan

- Dev service has the capture environment; production service does not.
- Capture filenames remain unique across daemon restarts.
- Saved audio is 16 kHz mono and replayable through ordinary WAV transcription.
- Directory and write failures warn without interrupting transcription.
- Diagnostics disabled behavior remains unchanged in other launch contexts.

## Done criteria

- [ ] Only the dev service enables normal-use capture.
- [ ] Startup logs disclose the local path and retain-until-delete policy.
- [ ] Production Dictate remains unchanged.
- [ ] Captures remain plain local WAVs with no corpus workflow.
- [ ] Full verification passes.

## STOP conditions

Stop and write a handback if:

- Production service or ordinary release behavior begins retaining audio.
- Capture becomes continuous or extends beyond an explicit dictation interval.
- A write failure can block transcription, insertion, or overwrite another file.
- The change requires a corpus manager, labels, automatic analysis, or identity inference.
- Retention semantics change from the disclosed retain-until-delete policy without explicit user review.
- A requested diagnostic recording lacks explicit `go` or `ready` after its contents are explained.
- A verification command fails twice after a reasonable fix.

## Maintenance notes

This directory is a manual debugging bank. It is not a product session store. If disk growth becomes a real problem, design a visible retention control rather than silently deleting files. If native stereo capture becomes useful, keep it dev-only and give it a distinct filename and storage disclosure.
