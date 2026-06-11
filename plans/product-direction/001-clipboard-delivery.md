# Plan 001: Deliver dictation to the clipboard through a typed delivery seam

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from e65b4661cfcf -- src/daemon.rs src/cli.rs src/lib.rs Cargo.toml`
> The gpui-rewrite-hardening effort (plans 003–005) intentionally modifies
> `src/daemon.rs` — read the live code, not just the excerpts below. If the
> transcription-delivery site has moved or changed shape beyond those plans'
> documented edits, treat it as a STOP condition.

## Status

- **Effort**: S–M
- **Risk**: LOW (additive; stdout remains the default)
- **Depends on**: none in this effort. Coordinate with
  `plans/gpui-rewrite-hardening/` 003–005 (same file `src/daemon.rs`);
  run after that track or rebase carefully.
- **Planned at**: revision `pkzmprvzlnsn` (git `e65b4661cfcf`), 2026-06-11

## Why this matters

Dictate currently prints formatted dictation to stdout and nothing else
(`src/daemon.rs:128`). The README names delivery as the next focus
(`README.md:21`: "real delivery targets such as copy, insert, and configured
output modes"). Every serious dictation app delivers text to where the user
is working; stdout is only useful when watching the daemon's terminal.
Clipboard delivery is the first real target: unlike keystroke injection it
works identically on every Wayland compositor (the data-control protocol is
universally implemented), needs no daemons or group permissions, and gives
the user a one-keystroke path (paste) to land text anywhere. Injection is a
separate spike (plan 002); this plan builds the seam both will share.

## Current state

- `src/daemon.rs:124-134` — the microphone worker's transcription loop is
  the only delivery site:

  ```rust
  match crate::transcription::transcribe(&recognizer, &utterance) {
      TranscriptionResult::Transcript(raw) => {
          let text = formatter.format(raw, &context);
          if !text.is_empty() {
              println!("{}", text.as_str());
          }
      }
      ...
  ```

- `src/cli.rs:13-22` — `Command::Daemon` takes no arguments today;
  `Command::Record` shows the clap pattern used (`Subcommand`, doc-comment
  help, `value_name`).
- `src/lib.rs` — module exports; a new module must be registered here.
- `Cargo.toml:8-25` — no clipboard crate. The GPUI overlay window is
  `KeyboardInteractivity::None` and never focused (`src/app.rs:124`), so
  GPUI's own clipboard (which rides the core data-device protocol and needs
  focus serials) is not usable — use the data-control protocol instead via
  the `wl-clipboard-rs` crate, which serves selections without a focused
  surface.
- Conventions: typed domain seams over stringly config (AGENTS.md);
  errors via `anyhow` with context (exemplar `src/daemon.rs:26-36`);
  wire enums get serde + round-trip tests (exemplar
  `src/dictation.rs`'s `DictationCommand` and `src/daemon.rs:206-218`).

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| Run live  | `just run daemon` (Wayland + mic)           | daemon ready line   |
| Read back | `wl-paste`                                  | the dictated text   |

## Scope

**In scope**:
- `src/delivery.rs` (new)
- `src/daemon.rs` (only the delivery call site and `Daemon` plumbing)
- `src/cli.rs` (a `--delivery` argument on the daemon subcommand)
- `src/lib.rs` (module export)
- `Cargo.toml` (the `wl-clipboard-rs` dependency)

**Out of scope** (do NOT touch):
- Keystroke/insertion delivery — plan 002 (spike) decides the mechanism.
- Settings/TOML persistence — plan 003 absorbs the CLI flag as a setting;
  the flag stays as a runtime override afterward.
- `src/text.rs`, `src/transcription.rs` — formatting is unchanged.
- Clipboard history, restore-after-paste, or middle-click PRIMARY selection
  — note as future options in Maintenance notes only.

## Steps

### Step 1: Add the delivery seam

Create `src/delivery.rs` with a typed target the daemon consumes:

- `pub enum DeliveryTarget { Stdout, Clipboard }` — `Clone, Copy, Debug`,
  plus `clap::ValueEnum` so the CLI parses it directly. `Stdout` is the
  `Default`.
- `pub fn deliver(target: DeliveryTarget, text: &str) -> Result<()>` —
  `Stdout` keeps today's `println!` behavior; `Clipboard` sets the Wayland
  clipboard via `wl_clipboard_rs::copy` (MIME `text/plain;charset=utf-8`).
  The daemon process is long-lived, so serving the selection from the
  crate's background mechanism is fine; consult the crate docs for the
  non-forking option suited to a resident process.
- On clipboard delivery, print a short status line to **stderr** in the
  daemon's existing voice (exemplar `src/daemon.rs:83`), e.g.
  "dictation copied to clipboard (N chars)". Never echo the full text to
  stderr — the clipboard is the delivery.
- A clipboard failure must not kill the worker loop: report via `eprintln!`
  and fall back to stdout delivery for that utterance so text is never lost.

**Verify**: `just check` → exit 0.

### Step 2: Wire the daemon and CLI

- `Command::Daemon` gains `--delivery <stdout|clipboard>` (clap `ValueEnum`,
  default `stdout`). Thread it `cli.rs → daemon::run(...) → Daemon →
  spawn_microphone_worker`, replacing the bare `println!` at
  `src/daemon.rs:128` with `delivery::deliver(...)`.
- `dictate daemon` with no flag behaves byte-for-byte as today.

**Verify**: `just check` → exit 0; `cargo run -- daemon --help` → shows the
delivery flag with both values.

### Step 3: Tests

In `src/delivery.rs`'s `#[cfg(test)] mod tests`:

- `DeliveryTarget` default is `Stdout`.
- clap value parsing round-trips both variants (pattern:
  `src/daemon.rs:206-218`'s wire-format test).

No test opens a real Wayland connection — clipboard behavior is verified
live in Step 4.

**Verify**: `just test` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0.

### Step 4: Live verification

On the real Wayland session:

1. `just run daemon -- --delivery clipboard` (check the Justfile arg
   passthrough; adjust invocation if `just run` doesn't forward flags).
2. `dictate record toggle`, speak a sentence, toggle again.
3. `wl-paste` → prints the formatted dictation.
4. Paste into a GUI app (e.g. a browser text field) → same text.
5. Kill and restart the daemon with no flag; confirm stdout delivery still
   works.

**Verify**: all five observations hold; record them in the PR description.

## Done criteria

- [ ] `just test` → all pass, including new delivery tests
- [ ] `cargo clippy --all-targets -- -D warnings` → exit 0
- [ ] `wl-paste` returns dictated text after a clipboard-delivery session
- [ ] Default behavior (no flag) unchanged: text on stdout
- [ ] Only in-scope files modified (`jj st`)

## STOP conditions

Stop if:

- The delivery site in `src/daemon.rs` no longer matches "Current state"
  beyond the documented gpui-rewrite-hardening edits.
- `wl-clipboard-rs` cannot serve a selection from the daemon without
  forking the process or holding a focused surface — handback with the
  exact API limitation; the fallback design (shelling to `wl-copy`) is a
  maintainer decision, not yours.
- The clipboard set succeeds but `wl-paste` returns stale/empty content on
  the maintainer's compositor (niri) — handback with the compositor and
  protocol versions (`wayland-info | rg data_control`).

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- Plan 002's insertion mechanism becomes a third `DeliveryTarget` variant;
  keep the enum the single place delivery choices live.
- Plan 003 (settings) makes the target persistent config; the CLI flag then
  overrides the setting — note the precedence in that plan's wiring.
- Future options deliberately deferred: restore-previous-clipboard after a
  timeout (racy on Wayland — see OpenWhispr issue #240 for the failure
  mode), PRIMARY-selection delivery, auto-paste. Each is a small extension
  of `deliver()`.
