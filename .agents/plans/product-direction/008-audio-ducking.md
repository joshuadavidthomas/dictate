# Plan 008: Duck system audio while recording

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 3c1eedcded87 -- crates/dictate/src/daemon.rs crates/dictate/src/settings.rs crates/dictate-desktop/`
> This plan was written against that revision. Plan 007 (live partials
> surface) touches `daemon.rs` too and may land first — its edits are
> confined to the overlay/partials paths. Read the live code; on a
> mismatch beyond that, treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: MED (mutates external audio state; the whole feature is a
  restore-discipline exercise)
- **Depends on**: none (implemented alongside 007 in the same working
  copy)
- **Planned at**: git `3c1eedcded87`, 2026-08-13
- **Completed at**: git snapshot `256fb711b983`, 2026-08-13

## Why this matters

System audio playing while dictating is distracting and bleeds into the
mic. Premium dictation apps duck playback during capture. The maintainer
wants a **slight, adjustable** dip — adjustable because the right amount
differs between music in the background and an active call (where the
user still needs to hear the other side, or may want no dip at all).

**Maintainer design decision (2026-08-13)**: dip the default sink by a
relative, configurable fraction while recording is live; a setting
controls the amount and `0` disables the feature entirely. Per-stream
role-aware ducking (dip music more than call audio) was considered and
deferred — it drags in stream-role policy for marginal gain.

## Current state

- Recording lifecycle in `crates/dictate/src/daemon.rs`
  (`run_microphone_worker`): recording becomes live at the
  `overlay.show(OverlayState::Recording)` site (~line 643, after the mic
  opens and the phase check passes). Every exit funnels through a small
  set of sites in the same worker loop: `MicSessionAction::Close`
  (cancel), `FinishStopping::Empty` (silent stop, ~line 657),
  `finalize_ready_dictation` (~line 670), the mic-open error path, and
  auto-stop (which arrives via `dictation` state, not a separate site).
  The worker also exits entirely on recognizer-init failure
  (`mark_unavailable`).
- `crates/dictate/src/settings.rs` — TOML settings with serde
  `deny_unknown_fields`, validation at load (`load_from_path`), and a
  doc-comment example block; follow the `partials_model` pattern for a
  new validated field.
- `crates/dictate-desktop` — owns desktop-environment side effects
  (focus observation, text delivery). Audio ducking is the same species
  of concern and belongs here, not in `dictate-speech` (mic capture) or
  a new crate (AGENTS.md: no shared/common crates).
- State-file precedent: `CapturePersistence::from_env`
  (`daemon.rs:750`) uses env-configured directories;
  `~/.local/state/dictate-dev/captures` appears in the dev service's
  environment. Use the same state-dir family for crash recovery.
- Audio stack on the target machine: PipeWire with `pipewire-pulse`
  (Arch). The daemon has no audio-output dependencies today; mic
  capture uses cpal.

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| Run live  | `just run daemon` (Wayland + audio playing) | dip + restore audible |

## Scope

**In scope**:
- `crates/dictate-desktop/` (new audio-ducking module: query, dip,
  restore the default sink)
- `crates/dictate/src/settings.rs` (new `duck_audio` setting)
- `crates/dictate/src/daemon.rs` (drive duck/restore at the existing
  recording transition sites; startup recovery)

**Out of scope** (do NOT touch):
- Per-stream / role-aware ducking policy.
- Pausing media players (MPRIS) — different feature.
- Any WirePlumber configuration or policy files.
- `crates/dictate-speech/` — recording state machine already exposes
  everything needed.

## Steps

### Step 1: Pick the control mechanism (bounded decision)

Evaluate, in order, and take the first that works headlessly on
PipeWire:

1. **`libpulse-binding`** against `pipewire-pulse`: mature crate, gives
   read-volume + set-volume + change notifications on the default sink.
2. **`pipewire-rs`**: native, but volume control is lower-level
   (`Props`/mixer manipulation); only if libpulse is unworkable.

Shelling out to `wpctl` is the fallback of last resort (precedent:
`wtype` is an external process) — prefer a library so failures are
typed, not parsed.

Record the choice and why in the PR description. Constraints on the
resulting module (whatever the mechanism):

- `dictate-desktop` exposes a small typed seam, e.g.
  `AudioDucker::duck(fraction) -> Result<DuckGuard>` /
  `restore(...)` — the daemon never sees libpulse types (AGENTS.md
  integration-boundaries rule).
- A ducking-specific Pulse query, update, or recovery failure degrades
  to a logged no-op: recording continues at the current output volume.
  Execution found that Dictate's existing microphone code explicitly
  uses CPAL's PulseAudio host, so a machine with no Pulse-compatible
  server cannot record at all. README requirements must state that
  product-wide dependency rather than claim ducking alone needs it.

**Verify**: `just check` → exit 0.

### Step 2: The `duck_audio` setting

- `duck_audio` in `Settings`: fraction of current volume to dip,
  `0.0` disables. Default **0.2** (a slight dip, per the maintainer).
  Validate range `0.0..=1.0` at load with an error message in the
  style of the `partials_model` errors (say what was wrong, show a
  valid example).
- Document it in the settings doc-comment example block and README's
  settings section.

**Verify**: `just test` → settings tests pass, including new
range-validation tests (valid, disabled, out-of-range).

### Step 3: Restore discipline

The core of the feature. Required behavior:

- **Duck** when recording becomes live (the
  `overlay.show(OverlayState::Recording)` site — not before the mic is
  confirmed open, so a failed open never ducks).
- **Restore** on every exit from live recording: manual stop, cancel,
  auto-stop, empty capture, mic stream error, and worker teardown.
  Prefer an RAII guard held by the worker for exactly the live-recording
  span over paired calls at N sites — the type system then proves no
  path leaks a duck. Restore happens at recording end, not after
  transcription (the user's music should come back while the final
  decode runs).
- **User-volume-change tolerance**: before restoring, re-read the sink
  volume; if it no longer matches the ducked value the user adjusted it
  mid-recording — skip the restore (never fight the user). This
  comparison needs a tolerance for volume quantization.
- **Crash recovery**: persist `{sink, pre-duck volume}` to a state file
  while ducked; remove it on restore. On daemon startup, if the file
  exists, apply the same skip-if-user-changed restore logic, then
  remove it. A daemon killed mid-dip therefore heals on next start.
- **Default-sink change mid-recording**: restore the sink that was
  ducked, not whatever is default at stop time.

**Verify**: `just check` → exit 0; `just test` → all pass.

### Step 4: Live verification

1. Play music; start recording → volume dips slightly (default 0.2);
   stop → volume returns exactly, while transcription is still running.
2. Cancel mid-recording → restore.
3. Auto-stop (shorten the cap via a debug knob if cheap, else skip and
   note) → restore.
4. Start recording, adjust volume manually, stop → your adjustment
   survives (no restore).
5. `duck_audio = 0` → no volume change at all.
6. Kill the daemon (`systemctl --user kill --signal=KILL dictate-dev`)
   mid-recording → restart → volume healed.
7. Force a ducking-only failure while leaving the existing Pulse mic
   host available → recording remains active and one ducking-unavailable
   line is logged. A missing `PULSE_SERVER` was also tested: CPAL's
   PulseAudio mic host becomes unavailable before ducking runs, which
   confirmed the product-wide server dependency now documented in the
   README.

**Verify**: observations recorded in "Completion evidence" below.

## Test plan

The testable core is the policy, not the audio server: extract
duck/restore decisions into plain functions — restore-or-skip given
(ducked volume, current volume, tolerance), state-file round-trip,
settings range validation — and unit-test those
(pattern: `src/dictation.rs:256+` plain-function tests). The libpulse
seam itself is verified live; do not mock the audio server.

**Verify**: `just test` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0.

## Completion evidence

Validated against two disposable Pulse null sinks at exact channel
volumes, then restored the original default sink and source:

- Default 20% duck changed both channels from `32768` (50%) to `26214`
  (40%); manual stop restored `32768` before the
  `transcribing captured audio` log line.
- Cancel restored both channels and removed `audio-duck.json`.
- A manual change to `42598` (65%) survived cancel; the guard logged that
  restore was skipped and removed the recovery record.
- Switching the default from sink A to sink B while recording restored A
  to 50% and left B at 60%.
- `duck_audio = 0` left both channels at `32768` and created no recovery
  record.
- SIGKILL left A at 40% with a recovery record; the service restart
  restored A to 50% and removed the record.
- A second daemon launched while A was ducked failed socket ownership,
  left A at 40%, and left the active recovery record intact. Startup
  recovery now runs only after socket ownership is acquired.
- A malformed recovery record made ducking log an unavailable warning;
  microphone capture and live hypotheses continued at 50%.
- Pulse volume updates now distinguish confirmed application, confirmed
  rejection, and a lost callback. An uncertain update is reconciled
  against the original, ducked, and user-changed volumes; if immediate
  recovery also fails, the live guard and state file both remain armed.
  Pure transition tests cover all three observed-volume states.
- Unloading a disposable monitor source did not make PipeWire end its
  active capture stream, so it could not provide a live mic-error case.
  The worker-local RAII guard covers stream error, auto-stop, empty
  capture, and teardown in source; SIGKILL separately covered the path
  where Rust destructors cannot run. The ten-minute auto-stop wait was
  skipped as allowed by Step 4.
- Setting a missing `PULSE_SERVER` proved that the current CPAL mic host
  also requires Pulse. The README now states this existing product
  dependency; ducking-specific failures remain nonfatal.
- `just fmt --check`, `just check`, `just test`, and
  `cargo clippy --locked --all-targets --all-features -- -D warnings`
  passed. After the full gates, focused desktop/UI/daemon tests passed;
  the uncertain-update fix then passed its focused tests and strict
  desktop clippy. The final installed build again ducked 50% to 40%,
  restored 50% on cancel, and removed its recovery record.

## Done criteria

- [x] `just test` → all pass
- [x] `cargo clippy --locked --all-targets --all-features -- -D warnings`
      → exit 0
- [x] Live run: dip on record, restore at recording end,
      user-adjustment respected, crash heals on restart
- [x] Ducking-specific failure → logged no-op, recording unaffected
- [x] 008 changes stay within its owned desktop, settings, daemon, Cargo,
      and README paths; 007 and 009 coexist in the working copy

## STOP conditions

Stop if:

- Neither libpulse-binding nor pipewire-rs can read+write sink volume
  reliably against pipewire-pulse — describe what failed before
  falling back to `wpctl` subprocess calls.
- Volume restore proves racy in a way tolerance can't cover (e.g.
  PipeWire reports stale volumes after set) — record the observed
  sequences.
- The RAII-guard shape can't span the worker loop's ownership structure
  without contorting it — the paired-calls fallback needs maintainer
  sign-off because it reintroduces leak-by-forgotten-path.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- Role-aware ducking (dip music more than calls) is the natural second
  layer if the flat dip proves too blunt; it would build on the same
  `AudioDucker` seam.
- If settings later grow live-reload, the duck fraction is read at
  duck time, so it picks up changes without daemon restarts for free.
- MPRIS pause-on-record is a sibling feature some apps offer; keep it
  separate from ducking if ever planned.
