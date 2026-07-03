# Formatter × Native ASR Punctuation Compatibility

**Source roadmap:** `.agents/ROADMAP.md` (Now #1, plus the "Spoken-punctuation fixture clips", "Model punctuation capability flag", and "Agentic feedback loop" standing-policy rows)
**Source feature artifacts:** `plans/product-direction/004-default-model-parakeet.md` (BLOCKED handback)
**Planned at:** 2026-07-03, working copy `rvskvrqq` / git `1697863e` (parent `fc9c3b87` "Curate transcription fixture corpus")
**Scope:** `src/text.rs` formatter, `src/cli.rs`, `tests/fixtures/`
**Planner:** roadmap-to-improve-plans session, 2026-07-03

## Purpose

Parakeet TDT 0.6B v2 won the plan-004 eval outright (complete >35s
transcripts, best accuracy on technical vocabulary, 0.75s short-utterance
decode) and is blocked as default by exactly one thing: its native
punctuation collides with spoken punctuation commands. Dictating
`Hello comma world period …` through the real daemon produced:

```text
Hello,, world,.

Thanks,. I use GPUI and Sherpa Onyx on Way.
```

This effort removes that collision and builds the verification substrate to
prove it stays removed. It is the critical path of the whole roadmap: the fix
unblocks the default flip (plan 004 re-run), which retires the product's
worst silent failure (30s Whisper truncation), which unblocks the
live-partials spike (plan 006).

## What Better Means

- Spoken punctuation commands and model-emitted native punctuation coexist:
  the handback's repro formats to `Hello, world.` instead of `Hello,, world,.`
  — with no regression to the 30 existing formatter tests.
- An agent (or CI, or Josh) can verify formatter behavior against **real
  model output** without a microphone, a human, or the daemon:
  `dictate transcribe <wav>` runs WAV → transcription seam → formatter
  headlessly.
- Command-word speech is represented in the committed fixture corpus, so the
  collision class that was found live, late, in Step 4 of an eval is caught
  by `just test` / `just test-integration` forever after.

## Current State

- `src/text.rs:295-298` — `OutputText::push_punctuation` trims trailing
  *spaces* only, then appends the command mark. Punctuation the ASR attached
  to the preceding word survives, producing `Hello,,` / `world,.`.
- The formatter already handles the *other* three collision shapes (verified
  by existing tests at `src/text.rs:864-958`): ASR punctuation attached to
  the command word itself, standalone punctuation tokens following a
  command/filler, and multi-word commands refusing to match across
  punctuation boundaries (`span_allows_match`, `src/text.rs:485-496`).
- `src/cli.rs` has only `daemon` and `record` subcommands; the only way to
  push audio through the transcribe→format pipeline is the live daemon with
  a real microphone.
- `tests/fixtures/` holds 7 read-prose clips (CMU ARCTIC, LJ Speech) —
  no command-word speech, so no committed fixture can exercise this bug.
- Public seams already exist for all of this: `dictate::audio::load_wav_utterance`,
  `dictate::transcription::transcribe`, `dictate::models::{model_by_id, ensure_downloaded}`,
  `dictate::text::DictationFormatter` (used exactly this way by `tests/integration.rs`).

## Desired End State

- `dictate transcribe <wav> [--raw] [--model <id>]` exists and is the
  documented headless verification loop for transcription/formatting work.
- The punctuation-command dedup rules are implemented in `OutputText` with
  the handback repro locked in as a characterization test.
- Self-recorded spoken-command fixtures are committed under
  `tests/fixtures/`, their **real** Parakeet raw transcripts captured via the
  CLI and locked into `src/text.rs` formatter tests, replacing the inferred
  seeds.
- Plan 004's Step 4 can be re-run with confidence (the re-run itself is out
  of scope here — it awaits the maintainer's ~1GB RSS decision).

## Source Summary

| Opportunity or slice | Source type | Audit category | Standards concern | Impact | Effort | Risk | Confidence | Source evidence |
|---|---|---|---|---|---|---|---|---|
| Headless `dictate transcribe` CLI | Roadmap (Now #1 slice; agentic-feedback-loop seed) | DX / direction | boundaries (CLI is a thin edge over existing seams) | Agents + maintainer iterate on formatter/model behavior without mic or daemon | S | LOW | HIGH | `src/cli.rs`, `tests/integration.rs:57-76` (proves the seam chain works) |
| Punctuation-command dedup vs native ASR punctuation | Roadmap Now #1 / plan 004 handback | correctness / direction | boundaries (model output is boundary data; formatter must not assume one punctuation style) | Unblocks Parakeet default flip → retires 30s truncation | S–M | MED (judgment calls in dedup rules) | HIGH | plan 004 handback repro; `src/text.rs:295-298` |
| Spoken-command fixture clips + real-audio lock-in | Roadmap standing policy ("Spoken-punctuation fixture clips") | tests | verification | The collision class becomes permanently regression-guarded | S (plus human recording time) | LOW | HIGH | `tests/fixtures/README.md` fixture contract; plan 004 handback |

## Plan Order

| Plan | Status | Audit category | Standards concern | Depends on | Ready for routine execution? | Needs deeper planning? | Autonomy boundary | Notes |
|---|---|---|---|---|---|---|---|---|
| [001-transcribe-cli](001-transcribe-cli.md) | **Done** (2026-07-03, Codex-implemented, reviewed + verified) | DX / direction | boundaries | None | Yes | No | Routine execution | Landed. Note: live eval against the *user's* config is blocked by a stale `osd_position` key in `~/.config/dictate/config.toml` (fails loudly by design); verified with a clean `XDG_CONFIG_HOME` |
| [002-command-punctuation-dedup](002-command-punctuation-dedup.md) | **Done** (2026-07-03, Codex-implemented with one correct STOP, reviewed + verified) | correctness | boundaries / domain-modeling | None (001 strengthens its verification) | — | No | — | Landed. Executor STOPped on a red/green split mismatch (Literal case mislisted as green in the plan); adjudicated as a plan bookkeeping error, plan amended, resumed to completion. **Unblocks the plan 004 re-run** (pending the ~1GB RSS decision) |
| [003-spoken-command-fixtures](003-spoken-command-fixtures.md) | Revised v2 (TTS-first), ready | tests | verification | 001 + 002 (done); fixture *commit* rides with the Parakeet flip (004 re-run) | Yes (capture + characterization now; commit gated on ordering) | No | Human approval only for committing TTS fixtures under the fixture rules | Human recording demoted to optional enrichment after the 2026-07-03 TTS spike validated the fix on real Parakeet output |

## Dependency Notes

- 001 and 002 are independently landable in either order; land 001 first so
  002's executor can sanity-check against real Whisper output (and real
  Parakeet output if the model is installed) instead of only unit tests.
- 003 hard-depends on 001 (the CLI is how raw Parakeet transcripts get
  captured) and on 002 (otherwise the new characterization tests would
  assert the broken behavior). It also needs Josh to record clips — schedule
  it as the trailing plan.
- The plan 004 re-run (default flip) is **not** part of this effort. It
  needs the maintainer's ~1GB RSS decision and re-executes an existing plan.

## Verification Baseline

- `just check` — compiles.
- `just test` — full unit suite including all formatter tests (30 in
  `src/text.rs` today).
- `just clippy` — `cargo clippy --all-targets --all-features -- -D warnings`.
- `just fmt` — nightly rustfmt.
- `just test-integration` — model-backed corpus gate (WER ≤ 8%, CER ≤ 3%,
  per-fixture insta snapshots). Requires the default model preinstalled or
  `DICTATE_MODEL_DIR`; see `tests/integration.rs:124-151`.

All five commands verified present in `Justfile` at planning time.

## Evals / Regression Checks

- The plan 004 handback repro as a formatter characterization test (002) —
  catches the dedup fix regressing or being weakened.
- Existing formatter tests must pass unchanged — they encode the
  already-working collision handling that 002 must not break.
- Committed spoken-command fixtures under the corpus gate (003) — catches
  future model/formatter changes reintroducing the collision, and (post
  Parakeet flip) any return of a punctuation style the formatter mishandles.
- `dictate transcribe` gives every future formatter/model plan a one-command
  agent-runnable check; its removal or breakage would itself be a regression
  (001 documents it in AGENTS.md).

## Autonomy Boundary

| Action type | Routine execution allowed? | Needs design review? | Needs human approval? |
|---|---|---|---|
| Implementing 001 as specified | Yes | No | No |
| Implementing 002's rule table as written | Yes | Confirm rule table first (cheap — table is in the plan) | No |
| Changing which marks count as sentence punctuation, or the replace-vs-append rule | No | Yes | No |
| Disabling spoken commands by default under any model | No | No | Yes — product decision, explicitly deferred |
| Recording/committing new fixture audio and choosing its license | No | No | Yes — Josh records and owns the license |
| Flipping `DEFAULT_MODEL_ID` | No | No | Yes — belongs to the plan 004 re-run, not this effort |

## Drift Checks Before Any Plan

- Re-read project instructions (`AGENTS.md`) and this index.
- Check current VCS state (`jj st`, `jj log`) against `Planned at`; this
  effort was planned with only `.agents/` artifacts in the working copy.
- Re-open the files each plan cites before editing; line numbers were
  verified 2026-07-03.
- Stop if `src/text.rs` formatter internals, the CLI shape, or the Justfile
  commands have changed materially.

## Deeper Planning Candidates

| Plan/opportunity | Why it needs depth | Suggested next artifact |
|---|---|---|
| None in this effort | 002's design fork is small and pre-analyzed; the rule table in the plan makes review a confirmation pass | — |
| Spoken-commands-off-by-default under punctuating models | Product/UX call; only relevant if 002's dedup proves insufficient in the plan 004 re-run | `user decision`, revisit during the 004 re-run |

## Standing Policies / Decisions

| Decision or policy | Why it should not be re-litigated | Where to record or enforce it |
|---|---|---|
| No model-punctuation capability flag (for now) | The dedup rules are model-agnostic string rules at the output layer; nothing in 002 needs to know which model produced the text. Promote to a catalog field only when some consumer genuinely branches on it (e.g. the Later `OnlinePunctuation` fallback) | Decision recorded in 002; roadmap's "Model punctuation capability flag" row stays parked |
| Formatter never branches on model identity | Roadmap rejected per-model if-branches in `text.rs` (leaks model identity across the module boundary); if a rule ever needs model behavior, it enters as an explicit policy input on `DictationContext` | This index + 002's rejected approaches |
| Agentic feedback loop | Every feature plan names how an agent verifies the behavior headlessly | 001 adds the sentence to `AGENTS.md`; this effort's plans each carry an agent-runnable verification |

## Considered and Rejected

| Idea | Audit category | Reason rejected | Revisit if |
|---|---|---|---|
| Per-model if-branches in `src/text.rs` | architecture | Leaks model identity across the module boundary (roadmap rejection, restated for executors) | Never — use a policy input on `DictationContext` instead |
| ~~Synthesized TTS clips for spoken-command audio~~ **REVERSED 2026-07-03** | tests | Original rejection conflated "generated-by-unknown-source" with self-generated-with-recorded-provenance, and assumed rather than measured the prosody concern. A Piper spike reproduced the collision on real Parakeet output and validated the 002 fix; `en_US-ljspeech` (public-domain dataset) satisfies the provenance bar. Now the primary path in 003 v2 | — (spike evidence in 003) |
| YouTube-ripped audio for committed fixtures | tests | Standard-license uploads are not redistributable; CC-BY uploads need per-video license verification plus ToS friction — strictly worse than public-domain-voice TTS | A creator-provided direct download of a clearly-licensed dictation demo |
| Disabling spoken commands by default under punctuating models | direction | Premature: the dedup fix likely makes both layers coexist; turning commands off is a product regression for command users | 002 lands but the plan 004 Step 4 re-run still fails formatter sanity |
| `--formatted` as an explicit flag on the CLI | DX | Formatted is the default output; a second mutually-exclusive flag adds surface with no information | A third output mode (e.g. JSON) ever justifies an `--output` enum |

## Deferred

| Idea | Why deferred | Trigger to revisit |
|---|---|---|
| Plan 004 re-run (default flip to Parakeet) | Needs maintainer's ~1GB RSS acceptance; separate existing plan | 002 lands + RSS decision |
| Long-form (>30s) fixtures | Break the corpus gate under the Whisper default (silent truncation → WER blowout) | Land with/after the Parakeet re-run (roadmap Next) |
| `OnlinePunctuation` restore fallback for uncased models | Only matters if non-punctuating models stay user-selectable after the flip | Catalog decision during/after 004 re-run |

## Reconciliation Log

- `2026-07-03` — Effort created from `.agents/ROADMAP.md` Now #1 with the
  spoken-punctuation-fixtures and agentic-feedback-loop standing-policy rows
  folded in. Evidence re-verified by direct read at `1697863e`: the
  formatter's remaining gap is narrower than the roadmap's phrasing —
  only *preceding-word* attached punctuation is unhandled
  (`src/text.rs:295-298`); command-word-attached and following-standalone
  punctuation are already covered by tests.
- `2026-07-03` — Adversarial Codex review of the bank. Confirmed the
  handback-repro trace byte-for-byte. Killed 002's v1 rule table with two
  blockers (provenance-blind trimming eats prior command marks and
  replacement/dictionary punctuation); 002 rewritten around a
  protected-length watermark in `OutputText`. Also fixed: 002's false
  claim that Literal mode never reaches `push_punctuation` (Literal +
  `PunctuationOnly` does), 002's miscitation of
  `starts_with_closing_punctuation` as covering quotes (it does not),
  001's settings-validation ordering (invalid configured model fails even
  with `--model` — decided and documented as intentional), 001 print/`Display`
  and `just run` details, and a new 003 STOP for models rendering spoken
  command words as literal symbols (breaks both WER scoring and command
  matching). R4 (line breaks keep preceding punctuation) was challenged but
  retained — 003's captured-audio STOP is the evidence gate.
- `2026-07-03` — 001 executed (Codex) and landed. Environment finding: the
  maintainer's real `~/.config/dictate/config.toml` carries a stale
  `osd_position` key, so `settings::load()` (and therefore the daemon and
  the new CLI) fails loudly until it is removed; verified with a clean
  `XDG_CONFIG_HOME` instead. Real Parakeet raw output captured for a prose
  fixture: `Author of The Danger Trail, Philip Steeles, etc.` (native
  punctuation + capitalization confirmed).
- `2026-07-03` — 002 executed (Codex). Executor correctly STOPped after
  Step 1: the Literal + `PunctuationOnly` case was red pre-fix
  (`write,, then`) while the plan listed it as expected-green. Adjudicated
  as plan bookkeeping error (the case input carries attached native
  punctuation, so it exhibits the R1 bug — confirming R5); plan amended;
  resumed and completed. 79/79 tests, characterization snapshot formats the
  handback repro to `Hello, world.\n\nThanks. I use GPUI and Sherpa Onyx on
  Way.` The formatter STOP from plan 004 is now retired pending the Step 4
  re-run on real command audio (003 + the 004 re-run).
- `2026-07-03` — TTS spike (user seed: avoid self-recording). Piper
  `en_US-lessac-medium` clips through `dictate transcribe`: the collision
  reproduced on real Parakeet output (`Is this working question mark? Yes,
  exclamation mark …`) and the 002 fix formatted it correctly
  (`Is this working? Yes! …`) — end-to-end validation without human audio.
  Also measured: flat TTS prosody under-triggers Parakeet punctuation;
  whisper-base mangles the synthetic voice on some scripts (ordering:
  commit fixtures with/after the Parakeet flip); both models ITN numbers
  (scripts must avoid them). 003 rewritten v2 TTS-first with
  `en_US-ljspeech` (public-domain dataset) for committed fixtures; the
  TTS rejection in this index reversed with reasons; human recording
  demoted to optional enrichment.
