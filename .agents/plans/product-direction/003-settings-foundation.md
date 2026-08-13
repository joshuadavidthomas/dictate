# Plan 003: TOML settings that make the dormant formatter and model catalog reachable

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from e65b4661cfcf -- src/daemon.rs src/models.rs src/text.rs src/cli.rs src/lib.rs Cargo.toml`
> Plans 001 (delivery flag) and the gpui-rewrite-hardening track (003–005)
> touch `src/daemon.rs` first — read the live code. On a structural
> mismatch beyond those plans' documented edits, treat it as a STOP
> condition.

## Status

- **Effort**: M
- **Risk**: LOW–MED (new config surface on the daemon startup path;
  mitigated by defaults-when-absent and strict validation)
- **Depends on**: 001 (absorbs its `--delivery` flag as a setting).
  Coordinate with gpui-rewrite-hardening 003–005 (same `src/daemon.rs`).
- **Planned at**: revision `pkzmprvzlnsn` (git `e65b4661cfcf`), 2026-06-11

## Why this matters

Dictate ships a 16-model catalog (`src/models.rs:230-345`), a full
formatting system with seven modes, custom dictionaries, and replacement
rules (`src/text.rs:25-175`) — and none of it is reachable at runtime:
`src/daemon.rs:109-113` hardcodes `default_model()` and
`DictationContext::default()`. PLAN.md:199 lists "Settings contract and
TOML persistence" as a behavior to keep from the old app, and PLAN.md:216
demands "typed settings APIs instead of stringly settings access". This
plan is the unlock for most of the product surface: model choice, mode,
dictionary, replacements, and delivery target become user-configurable, and
later features (app-aware profiles, overlay position, insertion) get a
place to live.

## Current state

- `src/daemon.rs:107-114` — the worker thread hardcodes everything:

  ```rust
  let model = default_model();
  let model_dir = model.ensure_downloaded()?;
  let recognizer = model.create_recognizer(&model_dir)?;
  let formatter = DictationFormatter;
  let context = DictationContext::default();
  ```

- `src/models.rs:25` — `DEFAULT_MODEL_ID = ModelId::new("whisper-base-en")`;
  `src/models.rs:127-129` — `model_by_id(&str) -> Option<&'static
  ModelCatalogEntry>` is the validation hook for a configured model id.
- `src/models.rs:131-135` — `models_dir()` shows the `directories` crate
  pattern (`ProjectDirs::from("", "", "dictate")`); config belongs in
  `dirs.config_dir()` (`~/.config/dictate/config.toml`), data already goes
  to `dirs.data_dir()`.
- `src/text.rs:75-121` — `DictationContext` is built with a fluent API
  (`new(mode)`, `with_dictionary`, `with_replacement_rules`,
  `with_spoken_formatting`); `DictationMode` enum at `src/text.rs:25-33`
  (Raw, Literal, Message, Email, Note, Technical, Command).
  `CustomDictionary` (`src/text.rs:124`) and `ReplacementRule`
  (`src/text.rs:162`) — read their constructors for the exact shapes.
- Plan 001 added (if executed first, as ordered): `src/delivery.rs` with
  `DeliveryTarget` and a `--delivery` CLI flag.
- `Cargo.toml` — serde is present; **no `toml` crate yet**.
- Conventions: typed seams, no stringly config (AGENTS.md); errors via
  `anyhow` with actionable messages; serde derive enums use
  kebab/lowercase wire names (exemplar: `DictationCommand` in
  `src/dictation.rs`).

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| Run live  | `just run daemon`                           | daemon ready line   |

## Scope

**In scope**:
- `src/settings.rs` (new)
- `src/daemon.rs` (replace the hardcoded model/context with settings)
- `src/cli.rs` (flag-over-setting precedence for `--delivery`)
- `src/lib.rs`, `Cargo.toml` (module export; `toml` dependency)

**Out of scope** (do NOT touch):
- A settings **UI** — config file only; a GPUI settings window is future
  direction work.
- Hot-reload/watching the file — load at daemon startup only; restart to
  apply. Note it in the README if you touch docs at all (you shouldn't).
- Overlay position, audio device selection, hotkey configuration — listed
  in PLAN.md:199 but each needs plumbing that doesn't exist yet; the
  settings *struct* may reserve nothing for them (add fields when the
  consumers land — no speculative config, per repo philosophy).
- `src/text.rs` and `src/models.rs` internals — consume their public APIs.

## Steps

### Step 1: Define typed settings with serde + TOML

`src/settings.rs`:

- `pub struct Settings` with exactly the fields that have working consumers
  today: model id, dictation mode, spoken formatting (optional override),
  dictionary entries, replacement rules, delivery target.
  `#[derive(Debug, Deserialize, PartialEq)]` with `#[serde(default,
  deny_unknown_fields)]` — a typo'd key must be an error, not silence.
- `Settings::default()` must reproduce today's hardcoded behavior exactly
  (`default_model()`, `DictationContext::default()`, stdout delivery).
- `pub fn load() -> Result<Settings>` — resolve
  `ProjectDirs.config_dir()/config.toml` (pattern: `src/models.rs:131-135`);
  missing file → `Ok(Settings::default())`; present-but-invalid → an error
  naming the file path, the bad key/value, and one example of a valid value.
- Conversion seams, not stringly passthrough:
  `fn model(&self) -> Result<&'static ModelCatalogEntry>` validating via
  `model_by_id` (error lists valid ids from `ModelCatalogEntry::all()`),
  and `fn dictation_context(&self) -> DictationContext` building through
  the fluent API at `src/text.rs:82-115`.
- Serde representations for `DictationMode`/`SpokenFormatting`/
  `DeliveryTarget`: derive on the source enums if cheap, or mirror enums in
  `settings.rs` mapped at the seam — pick whichever keeps `text.rs`
  untouched per Scope (mirroring is acceptable here; it's a boundary type,
  not a compatibility shim).

Sketch of the file a user writes (document it in a doc comment on
`Settings`):

```toml
model = "parakeet-tdt-0.6b-v2-int8"
mode = "technical"
delivery = "clipboard"

[[dictionary]]
spoken = "gee pee you eye"
written = "GPUI"

[[replacements]]
spoken = "my email"
written = "josh@joshthomas.dev"
```

**Verify**: `just check` → exit 0.

### Step 2: Wire the daemon

In `src/daemon.rs`: load settings once in `Daemon::start` (or pass into
`run`), fail daemon startup with the actionable error on invalid config
(a daemon that silently ignores broken config is worse than one that
refuses to start), and use `settings.model()?` /
`settings.dictation_context()` in `spawn_microphone_worker` in place of
the hardcoded lines at `src/daemon.rs:109-113`. CLI `--delivery` (plan 001)
overrides the setting when given; document the precedence in the flag's
help text.

**Verify**: `just check` → exit 0; `just run daemon` with no config file →
identical startup messages to before.

### Step 3: Tests

In `src/settings.rs`'s tests module:

- Full TOML round-trip: the sketch above parses to the expected struct.
- Missing file → defaults equal today's behavior (assert model id ==
  `DEFAULT_MODEL_ID`, mode == Message, delivery == Stdout).
- Unknown key → error mentioning the key (`deny_unknown_fields`).
- Bad model id → error message contains at least one valid catalog id.
- Dictionary/replacement entries land in the built `DictationContext`
  (assert via formatting a phrase, pattern: the `format` helper at
  `src/text.rs:522-527`, or via `DictationContext` equality).

**Verify**: `just test` → all pass;
`cargo clippy --all-targets -- -D warnings` → exit 0.

### Step 4: Live verification

1. Write `~/.config/dictate/config.toml` with a dictionary entry and
   `mode = "technical"`; restart the daemon; dictate a phrase containing
   the spoken form; confirm the written form is delivered.
2. Break the file (bogus model id); confirm the daemon refuses to start
   and the message names the file, the value, and valid alternatives.
3. Delete the file; confirm defaults.

**Verify**: all three observations recorded in the PR description.

## Done criteria

- [x] `just test` → 56 passed, including 6 settings tests
- [x] `cargo clippy --all-targets -- -D warnings` → exit 0
- [x] `rg -n "default_model\(\)" src/daemon.rs` → no hits (command
      produced no output and exited 1)
- [x] Daemon with no config file behaves exactly as before: with
      `XDG_CONFIG_HOME=/tmp/dictate-empty-config`, `just run daemon` printed
      the same daemon-ready and transcription-ready startup lines before the
      timeout stopped the long-running daemon
- [x] Only in-scope files modified before plan status bookkeeping (`jj st`):
      `Cargo.toml`, `src/cli.rs`, `src/daemon.rs`, `src/lib.rs`,
      `src/settings.rs`

## STOP conditions

Stop if:

- The code at the "Current state" locations doesn't match beyond plan 001's
  and the hardening track's documented edits.
- Exposing serde on `text.rs` types without touching `text.rs` proves
  impossible AND mirroring feels like real duplication (>3 mirrored types)
  — that's a design fork on where the settings boundary sits.
- Settings turn out to be needed by the GPUI side (`src/app.rs`) to do
  this cleanly — overlay config is explicitly out of scope; handback.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Maintenance notes

- UX decision from plan 001 follow-up: settings should define the persistent
  default delivery target, while future record-command flags should be
  per-utterance overrides latched when recording starts. Avoid a hidden
  daemon-wide "current delivery mode" switch as the primary user model.
- Plan 004 (default model) and plan 002's follow-up (insert delivery) add
  values, not mechanisms — they should only touch defaults/variants here.
- App-aware profiles (deferred direction work) will want per-app
  `DictationContext` overrides; the natural shape is a `[profiles.<app>]`
  table that overlays the base settings — design then, not now.
- Reviewers: the failure mode to scrutinize is partial config — a file
  with only `mode = "email"` must inherit every other default.
