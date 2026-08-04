# Plan 008: Let the user choose the input device

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from 8bbf8294 -- crates/dictate-speech/src/mic.rs crates/dictate/src/settings.rs crates/dictate/src/cli.rs crates/dictate/src/daemon.rs`
> Plans 002/004/005 modify `mic.rs`/`daemon.rs` first — this plan is
> sequenced last partly for that reason. Read the live code; STOP on
> structural mismatch beyond those plans' changes.

## Status

- **Effort**: M
- **Risk**: MED (public settings contract + capture-path change)
- **Depends on**: none functionally; execute after 002, 004, 005 (file overlap)
- **Planned at**: revision `nootnkmorwsk` (git `8bbf8294`), 2026-08-04

## Why this matters

Dictate records from `default_input_device()` unconditionally. The 2026-08
incident report literally opened with "the dock/headset is still the default
input" — when the system default is the wrong mic, Dictate has no recourse
except the user reshuffling PipeWire defaults. A dictation tool needs: a way
to *see* which devices exist (headlessly, per the repo's debug doctrine) and
a setting to pin one, with an explicit, logged fallback to the system
default when the pinned device is absent (headsets unplug; the daemon is
resident).

## Current state

- `crates/dictate-speech/src/mic.rs:84-89` — `capture()`:

  ```rust
  let host = cpal::default_host();
  let device = host.default_input_device()
      .ok_or_else(|| anyhow!("no default input device found"))?;
  ```

  cpal's `HostTrait::input_devices()` yields an iterator of `Device`;
  `DeviceTrait::name()` → `Result<String>`.
- `crates/dictate/src/settings.rs:40-50` — `Settings` struct:
  `#[serde(default, deny_unknown_fields)]`, fields `model`, `mode`,
  `spoken_formatting`, `dictionary`, `replacements`, `delivery`,
  `shortcuts`; accessor methods below (e.g. `push_to_talk()` at line 115
  returning `Option<&str>` — match that shape). The doc comment at the top
  of the file shows the example TOML users copy; extend it.
- `crates/dictate/src/cli.rs:57+` — `enum Command` (clap `Subcommand`):
  variants `Daemon`, `Record`, `Paste`, `Dismiss`, `Transcribe`, `Debug`;
  dispatch at `cli.rs:133-147`.
- `crates/dictate/src/daemon.rs:580-588` — the daemon's `capture(...)`
  call site (inside `run_microphone_worker`). `dictate-dev` also calls `capture`
  for its live microphone preview and passes no requested device. Settings are loaded at daemon
  startup (`settings::load()`, used from `cli.rs`/`daemon.rs`); a device
  change requires a daemon restart, same as every other setting.
- Crate responsibilities (`AGENTS.md`): device enumeration/capture is
  `dictate-speech` (`mic.rs`); settings and CLI are the `dictate` binary.
  Keep the boundary: `dictate-speech` exposes capabilities, `dictate` wires
  configuration into them.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Workspace tests | `just test` | all pass |
| Speech-crate tests | `cargo test -p dictate-speech` | all pass |
| Typecheck | `just check` | exit 0 |
| Lint | `just lint` | exit 0 |
| Devices smoke | `cargo run -p dictate -- devices` | lists ≥1 input device, marks the default |

## Scope

**In scope**:
- `crates/dictate-speech/src/mic.rs` — device enumeration + selection
- `crates/dictate-speech/src/lib.rs` — exports
- `crates/dictate/src/settings.rs` — `input_device` key
- `crates/dictate/src/cli.rs` — `devices` subcommand
- `crates/dictate/src/daemon.rs` — thread the setting into `capture`
- `crates/dictate-dev/src/screens/overlay.rs` — update the debug-only capture call
  for the clean-break signature; it passes no requested device

**Out of scope**:
- Live device switching / settings hot-reload — restart semantics match all
  existing settings.
- The debug window or any UI for choosing devices.
- PipeWire-specific routing (node targets, `WirePlumber` rules) — name-based
  cpal selection only.

## Steps

### Step 1: Device enumeration + selection in `dictate-speech`

In `mic.rs`:

- `pub fn list_input_devices() -> Result<Vec<InputDeviceInfo>>` where
  `InputDeviceInfo` carries at least `name: String` and `is_default: bool`
  (compare against `default_input_device()`'s name). Export both from
  `lib.rs`.
- Extend `capture` to take the requested device:
  `capture(output_sample_rate, requested_device: Option<&str>, handler)` —
  clean break, update the call sites; do not add a parallel
  `capture_with_device` variant (repo rule: no compatibility shims).
- Selection semantics, factored as a pure function over names so it's unit
  testable without hardware (cpal `Device` is not constructible in tests):

  ```rust
  enum DeviceSelection { Requested(usize), FallbackDefault { requested: String } }
  fn select_input_device(requested: Option<&str>, available: &[String]) -> DeviceSelection
  ```

  Exact name match; on miss, fall back to the default device and make the
  capture log (plan 004's device-name line) show the fallback explicitly:
  `requested input device "X" not found; using default "Y"`. A missing
  requested device must not fail dictation — the resident daemon outliving
  an unplugged headset is the normal case, not an error.

**Verify**: `cargo test -p dictate-speech` → all pass, including new
selection-semantics tests.

### Step 2: `input_device` setting

`settings.rs`: add `input_device: Option<String>` to `Settings` (serde
default `None`), accessor `pub fn input_device(&self) -> Option<&str>`
(pattern: `push_to_talk()`). Extend the doc-comment example TOML with a
commented `# input_device = "..."` line. Follow the file's existing test
pattern for settings parsing (round-trip a TOML with and without the key).

**Verify**: `cargo test -p dictate` (or wherever settings tests live —
follow the existing module) → all pass.

### Step 3: Thread it through the daemon

Pass `settings.input_device()` down to the `capture(...)` call in
`run_microphone_worker` (`daemon.rs:580`). The value must reach the worker
the same way other settings-derived state does (see how `plan` /
`recording_delivery` are threaded into `spawn_microphone_worker`,
`daemon.rs:476-493` — match it).

**Verify**: `just check` → exit 0.

### Step 4: `dictate devices` subcommand

New `Command::Devices` in `cli.rs`: prints one line per input device —
name, a default marker, and a `(selected)` marker when it matches the
configured `input_device`. Human-readable lines on stdout; exit 0 with ≥1
device, nonzero with none (meaningful exit codes per the repo's debug
doctrine). Follow the existing dispatch pattern at `cli.rs:133-147`.

**Verify**: `cargo run -p dictate -- devices` → lists devices, marks the
default, exit 0.

## Test plan

- `mic.rs`: selection-semantics unit tests — exact match, miss→fallback,
  `None`→default; pattern: existing `mod tests` in `mic.rs`.
- `settings.rs`: parse tests with/without `input_device`, following the
  file's existing tests.
- CLI: if `cli.rs` has arg-parsing tests (see around `cli.rs:240`), add the
  `devices` variant to them.
- **Verify**: `just test` → all pass.

## Done criteria

- [ ] `just test` → all pass; `just check` → exit 0; `just lint` → exit 0
- [ ] `cargo run -p dictate -- devices` lists input devices with default +
      selected markers
- [ ] With `input_device` set to a nonsense name, the daemon still records
      (fallback) and logs the fallback line
- [ ] `just hawk` reviewed — new `dictate-speech` public items
      (`list_input_devices`, `InputDeviceInfo`, widened `capture`) are
      deliberate and consistent with crate responsibilities
- [ ] No files outside the in-scope list modified

## STOP conditions

Stop and write a handback if:

- cpal device names on this system are unstable across enumerations (e.g.
  duplicated or index-suffixed names that don't round-trip) — name-based
  selection would need a different key, which is a design fork.
- Threading the setting requires changing `dictate-speech`'s public surface
  beyond the widened `capture` signature and the two new exports.
- Plans 002/004/005 are unlanded and `mic.rs`/`daemon.rs` conflicts get
  nontrivial — re-sequence rather than merge blind.

## Maintenance notes

- Name-based matching is deliberate (cpal's only stable, user-legible
  handle). If PipeWire node ids ever become necessary, that's a new plan
  with a different settings key, not a widening of this one.
- Plan 007's "ship-behind-config" outcome, if it happens, would put a second
  audio key in settings — keep naming consistent (`input_device`, not
  `mic`/`audio_input` variants).
- The `devices` subcommand is intentionally plain-text; if agents need
  machine-readable output later, add `--json` then (with serde on
  `InputDeviceInfo`) rather than speculatively now.
