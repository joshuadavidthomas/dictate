# 001 — Headless `dictate transcribe <wav>` subcommand

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

**Source item:** `.agents/ROADMAP.md` Now #1 (first-strategic-slice clause) + "Agentic feedback loop" standing-policy row
**Effort index:** `plans/formatter-punctuation-compat/README.md`
**Planned at:** 2026-07-03, working copy `rvskvrqq` / git `1697863e`
**Depends on:** none
**Executor target:** routine execution ready — yes
**Source type:** roadmap
**Audit category:** DX / direction
**Standards concern:** boundaries — the CLI is a thin edge that parses arguments into calls on existing refined seams; it must not grow its own transcription or formatting logic
**Impact:** agents, CI, and the maintainer can push a WAV through the real transcribe→format pipeline in one command, with no microphone, daemon, or human; every subsequent plan in this effort (and the formatter/model plans after it) verifies against this
**Effort:** S
**Risk:** LOW — additive subcommand; no existing behavior changes
**Confidence:** HIGH — the exact seam chain is already exercised by `tests/integration.rs:57-101`
**Source direction:** "include a headless `dictate transcribe <wav> [--raw|--formatted]` subcommand (seams exist: `audio::load_wav_utterance` → transcription seam → formatter) so agents can iterate on the fix against real model output without a mic or human"

## Purpose

Today the only path from audio to formatted text is the live daemon plus a
real microphone. Plan 004's formatter STOP was discovered in Step 4 of a
live eval because there was no earlier, cheaper way to run real model output
through the formatter. This subcommand is that way — and it is the first
substrate of the roadmap's agentic-feedback-loop policy.

## What Better Means

One command turns a WAV file into either the raw recognizer hypothesis or
the formatted dictation, using the user's real settings, with a nonzero exit
code when no transcript is produced. Regression would be the subcommand
producing output that differs from what the daemon would deliver for the
same audio and settings (excluding delivery itself).

## Current-State Evidence

- `src/cli.rs:14-27` — `Command` enum has only `Daemon { delivery }` and
  `Record { command }`; `run()` dispatches to `dictate::daemon::{run,send}`.
- `src/audio.rs:11` — `pub fn load_wav_utterance(path: &Path) -> Result<CapturedUtterance>`;
  rejects non-mono, non-16kHz, empty audio with good errors.
- `src/transcription.rs:48-65` — `pub fn transcribe(&OfflineRecognizer, &CapturedUtterance) -> TranscriptionResult`,
  where `TranscriptionResult` is `Transcript(RawTranscript) | NoTranscript(TranscriptionFailure)`
  and each failure has a `message()`.
- `src/models.rs:126-133` — `default_model()`, `model_by_id(&str) -> Option<&'static ModelCatalogEntry>`;
  `src/models.rs:91-112` — `ensure_downloaded()` (downloads on first use,
  progress on stderr); `src/models.rs:114-123` — `create_recognizer(&Path)`.
- `src/settings.rs:52-61` — `Settings::model()` resolves the configured
  model with an error message that lists valid ids;
  `src/settings.rs:63-89` — `Settings::dictation_context()` builds the full
  `DictationContext` (mode, spoken formatting, dictionary, replacements).
- `src/text.rs:179-227` — `DictationFormatter::format(RawTranscript, &DictationContext) -> ProcessedDictation`.
- `tests/integration.rs:57-101` — proves the chain
  `load_wav_utterance → create_recognizer → transcribe` works end-to-end
  against committed fixtures.

## Desired End State

```
$ dictate transcribe tests/fixtures/cmu-arctic/arctic_a0001.wav --raw
<recognizer hypothesis on stdout>

$ dictate transcribe recording.wav
<formatted dictation on stdout, per ~/.config/dictate/config.toml>

$ dictate transcribe quiet.wav; echo $?
captured dictation was too short or too quiet     # on stderr
1
```

- `--model <id>` overrides the settings model (same validation/error style
  as `Settings::model()`); the model auto-downloads if missing, matching
  daemon behavior.
- Transcript on stdout only; all diagnostics (download progress, errors) on
  stderr — so agents can capture stdout as the answer.
- `AGENTS.md` gains one sentence pointing agents at this loop.

## Scope

- `src/cli.rs` — new `Transcribe` subcommand variant + a private helper
  function that orchestrates the pipeline via the public `dictate::` seams.
- `AGENTS.md` — one sentence under a suitable existing section.

## Out of Scope

- Any change to formatter, transcription gates, models, settings, daemon,
  delivery.
- A `--mode`/`--dictionary` override (settings already control these; add
  flags only when a real workflow demands them).
- JSON/structured output; batch/multi-file input.
- The daemon audio-injection path (`record start --from-file`) — Later item.

## Design Claim

Boundaries (`coding-standards` → `boundaries.md`): the CLI edge parses
arguments into calls on refined internal seams and translates the
`TranscriptionResult` into process-level vocabulary (exit code + stderr
message). No transcription or formatting knowledge may live in `cli.rs`;
if the helper needs something the seams don't offer, that is a STOP, not an
inline workaround.

## Architecture Diagnosis

N/A — additive thin edge over existing seams.

## Implementation Sequence

### Step 1 — Add the subcommand

In `src/cli.rs`, extend `Command`:

```rust
/// Transcribe a WAV file through the dictation pipeline without the daemon.
Transcribe {
    /// Path to a 16 kHz mono WAV file.
    #[arg(value_name = "WAV")]
    wav: PathBuf,
    /// Print the raw recognizer hypothesis instead of formatted dictation.
    #[arg(long)]
    raw: bool,
    /// Override the model configured in ~/.config/dictate/config.toml.
    #[arg(long, value_name = "MODEL_ID")]
    model: Option<String>,
},
```

### Step 2 — Orchestrate through the public seams

Private helper in `src/cli.rs` (bin side), roughly:

1. `let settings = dictate::settings::load()?;`
   Decided behavior: `settings::load()` validates the *configured* model id
   before returning (`src/settings.rs:136-138`), so a config pointing at an
   unknown model fails here even when `--model` names a valid one. That is
   intentional — `--model` overrides a valid config, it does not rescue a
   broken one, and the daemon fails the same way on the same config. Do not
   add a validation-bypassing load path.
2. Resolve the model: `--model` via `dictate::models::model_by_id`, erroring
   with the valid-id list on unknown ids (mirror the wording shape of
   `Settings::model()`, `src/settings.rs:53-60`); otherwise
   `settings.model()?`.
3. `let model_dir = model.ensure_downloaded()?;` then
   `model.create_recognizer(&model_dir)?`.
4. `let utterance = dictate::audio::load_wav_utterance(&wav)?;`
5. `match dictate::transcription::transcribe(&recognizer, &utterance)`:
   - `Transcript(raw)` + `--raw` → `println!("{}", raw.as_str())`.
   - `Transcript(raw)` otherwise → format with
     `DictationFormatter.format(raw, &settings.dictation_context())` and
     print `.as_str()`.
   - `NoTranscript(failure)` → `anyhow::bail!("{}", failure.message())` so
     the process exits nonzero with the reason on stderr.

   (`RawTranscript` and `ProcessedDictation` expose `as_str()` and do not
   implement `Display` — `src/transcription.rs:20`, `src/text.rs:15`.)

Check what `settings::load` / `models` re-exports the lib actually makes
public (`src/lib.rs`) before writing imports; `tests/integration.rs` and the
existing `cli.rs` imports show the pattern.

### Step 3 — Document the agent loop

Add one sentence to `AGENTS.md` (near the Commands section):
transcription/formatting behavior is verified headlessly with
`dictate transcribe <wav> [--raw] [--model <id>]` against
`tests/fixtures/` audio — prefer it over live-daemon testing.

## Verification

### Automated

- [ ] `just check` — compiles.
- [ ] `just test` — no existing behavior regressed.
- [ ] `just clippy` — clean.
- [ ] `just fmt` — formatted.

### Evals / Regression Checks

- [ ] With the default model preinstalled (same precondition as
  `just test-integration`):
  `just run transcribe tests/fixtures/cmu-arctic/arctic_a0001.wav --raw`
  prints a plausible hypothesis for "Author of the danger trail, Philip
  Steels, etc." and exits 0.
- [ ] Same command without `--raw` prints formatted output (sentence-cased,
  per default Message mode) and exits 0.
- [ ] `just run transcribe tests/fixtures/cmu-arctic/arctic_a0001.wav --model bogus`
  exits nonzero and the stderr message lists valid model ids.
- [ ] Stdout purity:
  `just run transcribe tests/fixtures/cmu-arctic/arctic_a0001.wav --raw 2>/dev/null`
  emits the transcript and nothing else.

### Manual

- [ ] None beyond the above — the point of this plan is that nothing needs
  a human.

## Autonomy Boundary

Routine execution may include:

- Everything in the implementation sequence, including small judgment on
  helper naming/placement within `src/cli.rs` and the exact `AGENTS.md`
  sentence.

Design review is required for:

- Any new public API on the `dictate` lib crate (should not be needed; the
  seams are public already).
- Any deviation from stdout-transcript/stderr-diagnostics separation.

Human approval is required for:

- Nothing.

## Drift Checks

Before editing, the executor must:

- [ ] Re-read this plan and the effort index.
- [ ] `jj st` / `jj log` — compare with `Planned at` (`1697863e`); this plan
  assumes `src/cli.rs` still has exactly the `Daemon`/`Record` variants.
- [ ] Re-open `src/cli.rs`, `src/audio.rs`, `src/transcription.rs`,
  `src/models.rs:91-133`, `src/settings.rs:52-89` and confirm the cited
  signatures.
- [ ] Confirm `just check|test|clippy|fmt` exist in `Justfile`.

## STOP Conditions

Stop and hand back if:

- any cited seam has a different signature or is not reachable from the
  binary crate;
- the pipeline cannot be orchestrated without adding logic beyond
  parse/dispatch/translate to `cli.rs` (boundary claim violated);
- validation commands fail before any change;
- the change grows beyond one reviewable PR.

## Rejected Approaches

- `--formatted` flag alongside `--raw` — formatted is the default; a second
  mutually-exclusive flag is surface without information (effort index).
- Locate-only model resolution (the integration harness's refuse-to-download
  stance, `tests/integration.rs:124-151`) — the CLI mirrors the daemon's
  auto-download UX instead; the harness's stance protects CI, the CLI serves
  interactive/agent use.
- Putting the orchestration in the lib crate (new `dictate::` module) — no
  second consumer exists; keep the composition at the binary edge until one
  does.

## Standing Policy Updates

This plan implements the first substrate of the roadmap's "Agentic feedback
loop" policy and records it in `AGENTS.md` (Step 3).

## Executor Notes

- `main.rs` → `cli::run()` returns `anyhow::Result<()>`; `bail!` is the
  correct nonzero-exit path — do not call `std::process::exit`.
- `ensure_downloaded` prints progress via `eprintln!` — that is already the
  correct stream; leave it alone.
- Match the repo's import style: one `use` per item, no nesting (see any
  `src/*.rs` header).
- Do not touch `src/delivery.rs` — delivery is the daemon's concern; this
  command prints, full stop.
