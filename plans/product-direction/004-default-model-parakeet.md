# Plan 004: Evaluate Parakeet as the default model and retire the 30-second ceiling

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from e65b4661cfcf -- src/models.rs src/dictation.rs README.md`
> gpui-rewrite-hardening plan 004 adds `MAX_DICTATION_DURATION` to
> `src/dictation.rs` and a 30s limitation note to `README.md` — this plan
> builds on those. If hardening 004 has NOT landed yet, STOP (execute it
> first; this plan's Step 3 amends its artifacts).

## Status

- **Effort**: S–M
- **Risk**: LOW–MED (default-behavior change for every dictation; gated by
  a measured eval, trivially revertible — one constant)
- **Depends on**: gpui-rewrite-hardening 004 (recording cap + README note).
  Independent of plans 001–003 in this effort; runs any time after that.
- **Planned at**: revision `pkzmprvzlnsn` (git `e65b4661cfcf`), 2026-06-11

## Why this matters

The default model is Whisper base.en (`src/models.rs:25`), whose ONNX
export decodes a fixed 30-second window — longer dictation is silently
truncated (documented and bounded by hardening plan 004). The catalog
already carries NVIDIA Parakeet TDT entries (`src/models.rs:290-309`), and
the 2026 research is one-sided: Parakeet TDT 0.6B v2 scores ~6.05% average
WER (vs ~Whisper-large quality, far above base.en) with native punctuation
and capitalization, runs faster than real time on CPU, and decodes up to
**24 minutes** in a single pass — Handy, Superwhisper, and VoiceInk all
moved their local default to Parakeet in 2025–2026
(sources: huggingface.co/nvidia/parakeet-tdt-0.6b-v2, k2-fsa sherpa-onnx
NeMo transducer docs, superwhisper.com/changelog,
tryvoiceink.com/docs/recommended-models). If the eval confirms it locally,
flipping one constant removes the product's worst silent failure and
upgrades accuracy on technical vocabulary — the maintainer's primary use.

## Current state

- `src/models.rs:25` — `pub const DEFAULT_MODEL_ID: ModelId =
  ModelId::new("whisper-base-en");` used via `default_model()`
  (`src/models.rs:122-125`).
- Candidate catalog entries (already wired with recognizer configs):
  - `parakeet-tdt-0.6b-v2-int8` (`src/models.rs:290`) — English, 0.6B int8
  - `parakeet-tdt-0.6b-v3-int8` (`src/models.rs:297`) — 25 languages
  - `parakeet-tdt-ctc-110m-int8` (`src/models.rs:304`) — small/fast English
- `ModelCatalogEntry::ensure_downloaded` (`src/models.rs:87-108`) — models
  auto-download on first daemon start; a default-model change means a new
  multi-hundred-MB first-run download.
- Hardening plan 004 artifacts this plan amends: `MAX_DICTATION_DURATION`
  in `src/dictation.rs` (suggested 120s there) and the README "30 seconds"
  limitation note.
- The formatter (`src/text.rs`) assumes punctuated, capitalized ASR output
  (hardening plan 002 hardened exactly that for Whisper). Parakeet also
  emits punctuated, capitalized text — same input class, but verify with
  real output in the eval.
- Transcript gating: `MIN_DICTATION_DURATION`, `MIN_DICTATION_RMS`
  (`src/transcription.rs:7-8`) are model-independent; unchanged.

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Check     | `just check`                                | exit 0              |
| Tests     | `just test`                                 | all pass            |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |
| Run live  | `just run daemon`                           | daemon ready line   |

## Scope

**In scope**:
- `src/models.rs` (the `DEFAULT_MODEL_ID` constant only)
- `src/dictation.rs` (the recording-cap constant, if the eval supports
  raising it)
- `README.md` (the limitation note from hardening 004)

**Out of scope** (do NOT touch):
- Streaming/online recognition — sherpa-onnx supports only Zipformer
  streaming models, which lose accuracy; rejected for now (see effort
  README).
- VAD-segmented continuous transcription — the meeting path (PLAN.md:120+),
  separate future work; with a 24-minute window, dictation doesn't need it.
- Removing the recording cap entirely — it remains the memory guardrail.
- Settings (plan 003) — the default is the catalog's business either way.

## Steps

### Step 1: Side-by-side live eval

On the real setup, for each of `whisper-base-en` (control),
`parakeet-tdt-0.6b-v2-int8`, and `parakeet-tdt-ctc-110m-int8`: temporarily
point `DEFAULT_MODEL_ID` at it (or use plan 003's settings if landed),
restart the daemon, and dictate the same fixed script — include normal
prose, technical vocabulary ("GPUI", "sherpa-onnx", "Wayland",
"jj describe"), and one utterance > 35 seconds. Record per model:

1. Transcript accuracy (word errors on the script, especially technical
   terms).
2. The >35s utterance: truncated or complete?
3. Time from `record stop` → text delivered (stopwatch or temporary
   `eprintln!` timestamps — remove after).
4. Daemon startup time (model load) and resident memory (`ps -o rss`).
5. Download size on first run.

Decision rule: prefer `parakeet-tdt-0.6b-v2-int8` if accuracy ≥ whisper
base.en on the script and decode latency feels instant (≤ ~1s for a
sentence); fall back to `parakeet-tdt-ctc-110m-int8` if 0.6B load
time/memory is unacceptable for a resident daemon; keep whisper-base-en
and STOP if Parakeet output quality is somehow worse.

**Verify**: a results table for all three models in the PR description.

### Step 2: Flip the default

Change `DEFAULT_MODEL_ID` (`src/models.rs:25`) to the winner. Nothing else
moves — `default_model()` and all call sites resolve through the catalog.

**Verify**: `just test` → all pass; `just run daemon` → downloads (first
run) and reaches "microphone ready"; a dictation round-trips with correctly
punctuated text.

### Step 3: Re-true the cap and the README

- With a 24-minute decode window, the 120s cap from hardening 004 is no
  longer protecting against silent truncation — only memory. Raise
  `MAX_DICTATION_DURATION` to a value the eval supports (suggest 10
  minutes ≈ 37 MB of f32 samples at 16kHz; keep the auto-stop behavior and
  its status line).
- Update the README limitation note: the 30s caveat now applies only when
  a Whisper model is selected; the default no longer has it. Keep it
  honest — Whisper models remain in the catalog.

**Verify**: `rg -n "30" README.md` → note correctly scoped to Whisper;
`just test` → cap tests updated and passing (the cap tests from hardening
004 parameterize the rate — adjust constants, not test logic).

### Step 4: Formatter sanity on real Parakeet output

Dictate the golden-test-style phrases from hardening plan 002 (e.g. a
two-sentence utterance, a "comma"-spoken phrase) with the new default and
confirm formatting matches expectations. If Parakeet's punctuation style
breaks a formatter assumption Whisper satisfied (e.g. no sentence-final
periods), that's a STOP — formatter changes are out of scope.

**Verify**: observed outputs recorded in the PR description.

## Done criteria

- [ ] Eval table for 3 models in the PR description
- [ ] `rg -n "DEFAULT_MODEL_ID" src/models.rs` → points at the winner
- [ ] `just test` → all pass
- [ ] `cargo clippy --all-targets -- -D warnings` → exit 0
- [ ] A >35s dictation transcribes completely (no silent truncation)
- [ ] Only in-scope files modified (`jj st`)

## STOP conditions

Stop if:

- Hardening plan 004 hasn't landed (drift check) — order matters.
- `create_recognizer` fails for a Parakeet entry (the catalog config has
  never been exercised end-to-end) — handback with the sherpa-onnx error;
  fixing recognizer config is `src/models.rs` surgery beyond a constant
  flip.
- Parakeet accuracy or formatting compatibility is worse than
  whisper-base-en in Step 1/4 — the default stays; record the eval and
  hand back.
- Memory/startup cost of the 0.6B model makes the resident daemon
  noticeably heavier and the 110M model also loses on accuracy — that
  trade-off is the maintainer's.

On stopping, write a **handback**: current state, desired outcome,
lingering questions. Descriptive, not prescriptive.

## Handback: stopped on formatter compatibility

- **Current state**: no source changes are left in the working copy. The
  trial `DEFAULT_MODEL_ID = "parakeet-tdt-0.6b-v2-int8"`, 10-minute cap,
  and README edits were reverted after the Step 4 STOP. The default remains
  `whisper-base-en`, the cap remains 120 seconds, and the README still
  scopes the 30-second limitation to the current Whisper default.
- **STOP reason**: Parakeet TDT 0.6B v2 passed the accuracy and >35-second
  eval, but its native punctuation broke the spoken-punctuation formatter
  sanity check. Dictating `Hello comma world period new paragraph thanks
  period. I use GPUI and sherpa onnx on Wayland.` through the real daemon
  produced:

  ```text
  Hello,, world,.

  Thanks,. I use GPUI and Sherpa Onyx on Way.
  ```

  This matches the plan's Step 4 STOP condition: Parakeet's punctuation
  style breaks a formatter assumption Whisper satisfied, and formatter
  changes are out of scope for this plan.

### Eval results

| Model | Cache/download size | Startup/RSS | 60s decode | >35s result | Accuracy notes | Decision |
|---|---:|---:|---:|---|---|---|
| `whisper-base-en` | 433M cached | 0.7s / 240MB RSS | 5.4s | Truncated at 30s with sherpa-onnx warning | Missed Wayland (`whalen`), GPUI (`GPUi`), sherpa-onnx (`Sherpa Onyx`), `jj describe` (`JJ to scribe`) | Keep only because Parakeet formatter compatibility failed |
| `parakeet-tdt-0.6b-v2-int8` | 631M cached | 2.8s / 1.0GB RSS | 6.5s | Complete; included final >35s sentence | Best transcript: Wayland/GPUI correct, complete long passage; still rendered sherpa-onnx as `Sherpa Onyx` and capitalized `JJ Describe` | Accuracy winner, blocked by formatter compatibility |
| `parakeet-tdt-ctc-110m-int8` | 104M download, 126M extracted | 1.2s / 250MB RSS | 2.0s | Complete; included final >35s sentence | Worse than v2: Wayland → `Weyland`, `I amm`, `JJ Dcribe`; fast and memory-light | Not the default unless v2's 1GB RSS is unacceptable |

Short-utterance decode latency on the first 10 seconds of the eval audio:
Whisper 1.38s, Parakeet v2 0.75s, Parakeet CTC 0.24s. Parakeet v2 is
fast enough for sentence-level dictation once loaded, despite the larger
resident model.

### Desired outcome

Add a small formatter-compatibility plan before re-running this default
flip: spoken punctuation commands need to ignore or de-duplicate ASR-attached
punctuation on neighboring words when the model emits both native punctuation
and command words. After that lands, re-run Step 4 and then flip the default
to `parakeet-tdt-0.6b-v2-int8` if the maintainer accepts the ~1GB resident
RSS.

### Lingering questions

- Is ~1GB RSS acceptable for the resident daemon if accuracy and long-form
  behavior are clearly better?
- Should Parakeet default mode keep spoken punctuation enabled after the
  formatter fix, or should settings offer a Parakeet-oriented default context
  later?
- Is `sherpa onnx` → `Sherpa Onyx` acceptable unless users add a dictionary
  entry, or should technical vocabulary defaults expand in a separate plan?

## Maintenance notes

- Once plan 003 (settings) lands, the default only matters for first-run;
  the eval table is the document users/maintainer consult when picking.
- Parakeet v3 (multilingual) was deliberately not defaulted: the maintainer
  dictates in English and v2 scores better on English. Revisit if
  multilingual matters.
- sherpa-onnx ships no streaming Parakeet; if/when one appears, the
  streaming-partials direction item (effort README, rejected-for-now)
  becomes worth re-opening.
- Reviewers: confirm the first-run download UX (progress lines from
  `src/models.rs:98-105`) is acceptable for a ~600MB archive.
