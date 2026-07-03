# 002 — Punctuation commands de-duplicate native ASR punctuation

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

**Source item:** `.agents/ROADMAP.md` Now #1; `plans/product-direction/004-default-model-parakeet.md` handback ("Desired outcome")
**Effort index:** `plans/formatter-punctuation-compat/README.md`
**Planned at:** 2026-07-03, working copy `rvskvrqq` / git `1697863e`
**Depends on:** none hard; land 001 first so real-model spot checks are one command
**Executor target:** routine execution ready — yes, once the rule table below is confirmed in review
**Source type:** roadmap + plan handback
**Audit category:** correctness / direction
**Standards concern:** boundaries — recognizer output is boundary data; the formatter must translate any model's punctuation style into one internal result, not assume the Whisper style it was hardened against
**Impact:** the sole blocker on the Parakeet default flip, which retires the 30s silent-truncation failure and unblocks the live-partials spike — the highest-leverage small change in the repo
**Effort:** S–M
**Risk:** MED — dedup rules carry judgment calls; wrong rules corrupt every dictation that mixes commands with model punctuation
**Confidence:** HIGH — the failure is reproduced byte-for-byte by the trace below; the fix point is a single function
**Source direction:** "spoken punctuation commands need to ignore or de-duplicate ASR-attached punctuation on neighboring words when the model emits both native punctuation and command words" (plan 004 handback)

## Purpose

Parakeet emits `Hello, comma world, period.` for the speech "Hello comma
world period". The formatter turns the command words into marks but never
reconciles them with the marks the model already attached, producing
`Hello,, world,.`. This plan makes explicit spoken commands *win over*
adjacent model-attached punctuation, so both layers can coexist.

## What Better Means

The plan 004 handback repro formats to clean text (expected output below),
all 30 existing `src/text.rs` tests pass unchanged, and no rule strips
punctuation the user's text legitimately owns (closing brackets/quotes,
mid-sentence marks far from any command). Regression = any existing
formatter test changing, or the characterization tests requiring weakening.

## Current-State Evidence

- `src/text.rs:295-298` — `OutputText::push_punctuation` trims trailing
  **spaces** only, then appends the mark. This is the entire gap.
- Already handled (do not re-implement, do not break):
  - `src/text.rs:864-873` — ASR punctuation attached to the command word
    itself is dropped (`"Hello comma, world period."` → `"Hello, world."`).
  - `src/text.rs:927-957` — standalone punctuation tokens *after* a command
    or filler are consumed (`consumed_with_following_punctuation`,
    `src/text.rs:511-524`).
  - `src/text.rs:485-496` — `span_allows_match` stops multi-word commands
    from matching across punctuation boundaries
    (`"That's a good question. Mark will answer."` stays intact).
- The unhandled shape, traced against current code: raw
  `Hello, comma world, period. New paragraph, thanks, period. I use GPUI and Sherpa Onyx on Way.`
  1. token `Hello,` pushed as text (trailing `,` kept — it is part of `raw`);
  2. token `comma` matches the command → `push_punctuation(",")` appends
     after `Hello,` → `Hello,,`;
  3. `world,` + command `period.` → `world,.`;
  4. `New paragraph,` matches the line command (`span_allows_match` allows
     trailing punctuation on the *final* word) → `\n\n` — correct already;
  5. `thanks,` + `period.` → `thanks,.`;
  6. `capitalize_sentences` produces exactly the handback output:
     `Hello,, world,.\n\nThanks,. I use GPUI and Sherpa Onyx on Way.`
  The inference reproduces the handback byte-for-byte, which is strong —
  but it is still an inference about Parakeet's raw output; plan 003
  replaces it with captured real transcripts.

## Desired End State

The same raw input formats to:

```text
Hello, world.

Thanks. I use GPUI and Sherpa Onyx on Way.
```

(`Sherpa Onyx` / `Way.` are ASR accuracy errors — dictionary territory,
untouched by this plan.)

## Scope

- `src/text.rs` — `OutputText::push_punctuation` (and, only if review says
  so, `push_line_break`); new unit/characterization tests.

## Out of Scope

- Flipping `DEFAULT_MODEL_ID` (plan 004 re-run; human RSS decision).
- Recording or committing audio fixtures (plan 003).
- Single-word-command false positives on content words ("a rough period." as
  a noun) — pre-existing ambiguity, unchanged by this plan.
- Filler-comma handling ("um, so" → "so"), paragraph chunking, bracketed
  artifacts — the "deterministic formatter increments" roadmap item, a
  separate batch.
- Any model-capability flag in the catalog (see Standing Policy Updates).

## Design Claim

Boundaries: the formatter is the anti-corruption layer between recognizer
output and delivered text. Model punctuation style is source-shape variance
that must be absorbed here, by explicit rules on the output text — never by
branching on model identity (rejected by the roadmap; restated below).

## Architecture Diagnosis

- **Current friction:** `OutputText` assumes the text before a command mark
  is punctuation-free — true for the Whisper style it was hardened against
  (hardening plan 002), false for natively-punctuating models.
- **Deepening direction:** the dedup rule lives inside `OutputText`, keeping
  the policy invisible to the token loop — callers still just
  `push_punctuation(mark)`.
- **Deletion test:** N/A (no module removed).
- **Locality / leverage claim:** one function owns the reconciliation; every
  current and future punctuating model is handled without new seams.
- **Recommendation strength:** Strong.
- **ADR conflicts:** none.

## Dedup Rule Table (the design-review surface)

> Amended 2026-07-03 after an adversarial Codex review of the v1 table.
> v1 trimmed trailing punctuation with no notion of where it came from,
> which would have eaten marks emitted by prior spoken commands
> ("exclamation mark question mark" → `?` instead of `!?`) and by
> replacement/dictionary written text (`company` → `Acme Inc.` + spoken
> "comma" → `Acme Inc,`). v2 fixes both with one mechanism: provenance
> via a protected-length watermark.

Review confirms or amends this table before implementation; everything else
in the plan is routine.

| # | Rule | Rationale |
|---|---|---|
| R1 | `OutputText` tracks a **protected length** (byte watermark into `text`). When pushing a punctuation-command mark: trim trailing spaces, then trim a trailing run of **sentence punctuation only** (`, . ? ! : ;`) **without descending below the watermark**, then append the command mark | The user explicitly spoke the mark; it wins over what the model guessed for the same prosodic pause — but only model-guessed marks are up for grabs |
| R2 | The watermark rises to `text.len()` after: (a) any command output (punctuation mark or line break), and (b) replacement/dictionary **written text** — but *not* after the raw-ASR trailing punctuation that `replacement_text` re-attaches (`src/text.rs:498-509`), which stays trimmable | Marks the user configured (`Acme Inc.`, `...` snippets) or explicitly spoke are content; marks the model attached to a replaced token are still pause guesses |
| R3 | Never trim through characters outside the sentence-punctuation set — brackets, quotes, apostrophes, anything else stops the trim. If output ends with `)`, "period" yields `).`; if it ends with `,"`, "period" yields `,".` (accepted limitation — reaching inside closers is out of scope). Note: `starts_with_closing_punctuation` (`src/text.rs:559-564`) covers `) ] }` but **not** quotes; define the sentence-punctuation set independently, do not reuse that helper | Non-sentence marks belong to content; trimming inside closing quotes is typographic guesswork v1 should not attempt |
| R4 | `push_line_break` keeps trailing punctuation as-is (current behavior) | A mark before a line break (`Hello,\n\n`) is plausible sentence punctuation; the repro shows this path already behaves acceptably. Plan 003's captured real audio is the evidence gate — its STOP routes back here if wrong |
| R5 | Rules are unconditional — they apply to all models and all modes whose pipeline reaches `push_punctuation` (Message/Email/Note/Technical/Command, **and Literal with `SpokenFormatting::PunctuationOnly`** — see `src/text.rs:768-777`; only Raw mode and `Disabled` spoken formatting are immune) | Under Whisper, `Hello, comma` → `Hello,` is also strictly better; no model flag, no mode special-casing |

Same-mark adjacency needs no special case: R1 trims the raw-ASR `.` and
appends the command `.` → one mark (`world. Period.` → `World.`).

Known accepted consequences (note in the PR description):

- If the model legitimately ended a sentence and the user *then* spoke a
  mark ("…done. Comma"), the command replaces the `.` with `,` — faithful
  to "the spoken command wins", and the rarer case.
- A raw-ASR ellipsis before an explicit command is consumed
  (`wait... comma` → `Wait,`) — the run came from the model; a *replacement*
  that produces `...` is watermark-protected and survives.
- Consecutive spoken commands concatenate exactly as today
  (`comma period` → `,.`; `exclamation mark question mark` → `!?`) — the
  watermark makes the second command unable to eat the first.

## Implementation Sequence

### Step 1 — Characterization tests first (red)

In `src/text.rs` tests, add the handback repro as an insta snapshot test:

```rust
#[test]
fn parakeet_native_punctuation_coexists_with_spoken_commands() {
    // Raw input inferred from the plan 004 handback (reproduces its broken
    // output byte-for-byte on the pre-fix formatter); plan 003 replaces it
    // with a captured real Parakeet transcript.
    insta::assert_snapshot!(
        format(
            "Hello, comma world, period. New paragraph, thanks, period. I use GPUI and Sherpa Onyx on Way.",
            DictationContext::new(DictationMode::Message),
        ),
        @"..."
    );
}
```

Plus targeted unit tests for each rule:

- R1 different-mark: `"world, period."` → `World.`
- R1 same-mark: `"world. Period."` → `World.` (also covers the model
  capitalizing the command word)
- R1 trailing run: `"wait... comma"` → `Wait,` (raw-ASR run trimmed)
- R2 command-mark protection: `"really exclamation mark question mark"` →
  `Really!?` (second command must not eat the first — passes today, must
  keep passing)
- R2 consecutive-command concatenation: `"comma period"` → `,.` (current
  behavior preserved)
- R2 replacement protection: dictionary/replacement `"company"` →
  `"Acme Inc."`, input `"company comma"` → `Acme Inc.,` (configured period
  survives; spoken comma appends)
- R2 replacement with re-attached ASR punctuation: input `"company, comma"`
  (model attached `,` to the replaced token) → `Acme Inc.,` (the re-attached
  `,` is trimmable, the written `.` is not)
- R3 bracket guard: `"(see above) period"` → `(See above).`
- R3 quote limitation (documents accepted output): `"world,\" period"` →
  `World,".`
- R4 line break: `"Hello, new paragraph thanks"` → `Hello,\n\nThanks`
- R5 Literal + PunctuationOnly: `"write, comma then"` with
  `DictationMode::Literal` + `SpokenFormatting::PunctuationOnly` →
  `write, then` (dedup applies on this path too; no sentence-casing in
  Literal)

(Message-mode cases sentence-capitalize, hence the leading uppercase.)
- R4 line break: `"Hello, new paragraph thanks"` → `Hello,\n\nThanks`

Run `just test` before touching the implementation and record which tests
fail. Expected red: the characterization snapshot, the R1 cases, the
replacement-with-re-attached-punctuation case (doubled marks), **and the
Literal + `PunctuationOnly` case** — its input has native punctuation on
the word before the command, so it exhibits the same R1 bug on that path
(which is R5's whole point). Expected already-green (they encode current
behavior the fix must preserve): the command-mark protection,
consecutive-command, plain replacement-protection, bracket, quote, and
line-break cases. If the red/green split differs, the trace is wrong —
STOP.

> Split amended 2026-07-03 after a correct executor STOP: the first
> version of this plan mislisted the Literal case as expected-green
> (pattern-matched against the existing Literal test, whose input has no
> attached punctuation). The observed pre-fix failure `write,, then`
> confirms the design rather than contradicting it.

### Step 2 — Implement the watermark in `OutputText`

`src/text.rs:270-315`:

- Add a `protected_len: usize` field to `OutputText` (byte index into
  `text`; trims never descend below it).
- `push_punctuation` (`:295-298`): after `trim_trailing_spaces`, pop
  trailing characters while they are in the sentence-punctuation set
  (`, . ? ! : ;`) **and** `text.len() > protected_len`; then push the mark
  and set `protected_len = text.len()`.
- `push_line_break`: unchanged behavior, but set
  `protected_len = text.len()` after appending (a later command must not
  trim across a break).
- Replacement path: the token loop currently builds one string via
  `replacement_text` (`:498-509`) and calls `push_text`. Split it so
  `OutputText` can protect the written portion but not the re-attached ASR
  trailing punctuation — e.g. a `push_replacement(written_with_leading,
  asr_trailing)` method that pushes the first part, raises the watermark,
  then appends the trailing part unprotected. Keep `replacement_text`'s
  spacing/leading behavior intact.
- Keep the sentence-punctuation set as a named helper (e.g.
  `is_sentence_punctuation`) adjacent to `starts_with_closing_punctuation`
  so the two classifications read together — but do not merge them; the
  sets differ (quotes are in neither).
- `trim_trailing_spaces` in the command path must also respect the
  watermark if a protected segment can end in a space (it cannot today —
  written text is pushed trimmed — but guard it or assert it).

### Step 3 — Green + snapshot review

`just test` — new tests pass with the expected outputs, all pre-existing
tests unchanged. `just clippy`, `just fmt`.

### Step 4 — Real-model spot check (best effort)

If `parakeet-tdt-0.6b-v2-int8` is installed (or downloadable in the
session): use plan 001's CLI with `--model parakeet-tdt-0.6b-v2-int8 --raw`
on any committed fixture to confirm the raw style (attached marks), then
`--model … ` without `--raw` to confirm formatting sanity on read prose.
Committed spoken-command audio does not exist until plan 003 — do not block
on this step; record what was and wasn't checked in the PR description.

## Verification

### Automated

- [ ] `just test` — all formatter tests pass, including the new
  characterization tests; zero changes to existing test expectations.
- [ ] `just check`, `just clippy`, `just fmt` — clean.

### Evals / Regression Checks

- [ ] The characterization snapshot is the standing eval: any future change
  that reintroduces doubled marks fails it.
- [ ] Existing tests `punctuation_commands_do_not_cross_sentence_boundaries`,
  `commands_drop_following_standalone_punctuation*`,
  `literal_and_raw_modes_preserve_spoken_commands_snapshot`, and
  `literal_mode_can_enable_punctuation_without_line_commands`
  (`src/text.rs:768-777`) guard against over-eager stripping. Note: Raw
  mode and `SpokenFormatting::Disabled` never reach `push_punctuation`,
  but **Literal + `PunctuationOnly` does** — the rules intentionally apply
  there.
- [ ] The protection tests (command-mark, replacement written-text) are the
  standing guard against any future "simplification" that removes the
  watermark and reverts to blind trimming.

### Manual

- [ ] None required; Step 4 is best-effort and headless.

## Autonomy Boundary

Routine execution may include:

- Everything in the sequence, given the rule table as written.

Design review is required for:

- Confirming or amending the rule table (one pass, before Step 2);
- Any rule change discovered necessary during implementation (e.g. R4
  turning out wrong against real output) — amend the table first, then code.

Human approval is required for:

- Nothing in this plan. (Turning spoken commands off by default under any
  model is a product decision, explicitly out of scope.)

## Drift Checks

Before editing, the executor must:

- [ ] Re-read this plan and the effort index.
- [ ] `jj st` / `jj log` vs `Planned at` (`1697863e`).
- [ ] Re-open `src/text.rs:270-315` and confirm `OutputText` still has the
  cited shape; re-run the trace mentally if the token loop changed.
- [ ] Confirm the five existing collision tests cited in Current-State
  Evidence still exist and pass.

## STOP Conditions

Stop and hand back if:

- Step 1's characterization test does **not** reproduce the handback's
  broken output on the unmodified formatter — the inference about
  Parakeet's raw style is then wrong, and the fix needs real captured
  transcripts first (pull plan 003's capture step forward);
- implementing R1 breaks any existing test — the rule interacts with
  something this plan didn't map;
- the fix seems to require knowing which model produced the text;
- validation commands fail before changes.

## Rejected Approaches

- **Provenance-blind trimming (this plan's v1 rule table)** — trimmed any
  trailing sentence punctuation before a command mark, regardless of
  origin. Killed by adversarial review with two counterexamples: a second
  spoken command eating the first (`exclamation mark question mark` → `?`),
  and command dedup eating replacement/dictionary punctuation
  (`Acme Inc.` + "comma" → `Acme Inc,`). The watermark is the minimal
  provenance mechanism that fixes both.
- **Per-model if-branches in `src/text.rs`** — leaks model identity across
  the module boundary; roadmap-rejected. If a rule ever genuinely needs
  model behavior, it enters as an explicit policy input on
  `DictationContext`.
- **Same-mark-only dedup** — too weak; leaves `world,.` (the commonest
  failure in the repro, where the model guessed `,` and the user said
  "period").
- **Dropping ASR punctuation at tokenization when any command is nearby** —
  moves the rule away from the one place that knows what is being emitted
  (`OutputText`), and breaks the already-working attached/standalone
  handling.
- **Disabling spoken commands under punctuating models** — product
  regression for command users; deferred, only revisited if this fix fails
  the plan 004 Step 4 re-run.

## Standing Policy Updates

Decision: **no model-punctuation capability flag.** The rules are
model-agnostic output-text rules (R5); nothing here branches on the model.
The roadmap's "Model punctuation capability flag" row stays parked until a
consumer genuinely needs it (e.g. the Later `OnlinePunctuation` fallback).
Recorded in the effort index.

## Executor Notes

- Write the tests first and watch them fail — the whole plan hinges on the
  trace being right, and Step 1 is what proves it.
- `push_punctuation` receives only `&'static str` single marks from
  `punctuation_command_at` (`src/text.rs:384-405`); do not generalize the
  signature.
- `capitalize_sentences` runs after the token loop — expected outputs in
  tests must account for it (that is why `thanks.` appears as `Thanks.`).
- Keep test style consistent: plain `assert_eq!` for single-rule tests,
  insta inline snapshots for multi-line outputs, matching the existing file.
- Do not touch `span_allows_match`, `consumed_with_following_punctuation`,
  or tokenization — the changes live in `OutputText` plus the replacement
  push site in the token loop (`src/text.rs:199-207`); nothing else.
