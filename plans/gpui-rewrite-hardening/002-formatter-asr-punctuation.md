# Plan 002: Make the dictation formatter punctuation-safe on real ASR output

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving on.
> If anything in "STOP conditions" occurs, stop and write a handback —
> do not improvise. When done, update this plan's status row in the
> effort README.
>
> **Drift check (run first)**:
> `jj diff --from dd6db2c175a3 -- src/text.rs`
> If `src/text.rs` has changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Effort**: M
- **Risk**: MED (core text path — every dictation flows through it; mitigated
  by golden tests)
- **Depends on**: none (001 recommended first so CI gates the PR)
- **Planned at**: revision `mtnsrkmyruyz` (git `dd6db2c175a3`), 2026-06-11

## Why this matters

The default transcription model is Whisper, and Whisper emits **punctuated,
capitalized** text ("That's a good question. Mark will answer."). The
formatter in `src/text.rs` matches spoken commands, dictionary terms, and
replacements on punctuation-stripped token keys, then discards the matched
tokens' raw forms. Two silent-corruption bugs follow:

1. **Attached punctuation is dropped on replacement.** In Technical mode,
   "I prefer GPUI." formats to "I prefer GPUI" — the sentence-final period is
   lost every time a replaced term ends a clause. Same for custom dictionary
   terms and replacement rules.
2. **Commands match across sentence boundaries.** "That's a good question.
   Mark will answer." matches the spoken command `question mark` and formats
   to "That's a good? Will answer." — words are deleted and replaced with
   punctuation. Likewise "Something new. Paragraph two is next." triggers
   `new paragraph`.

These corrupt the core output of the product, invisibly. The existing test
suite never catches this because every test input is lowercase and
unpunctuated — unlike anything the ASR actually produces.

## Current state

All in `src/text.rs` (623 lines). The pipeline: `DictationFormatter::format`
normalizes whitespace, tokenizes, then walks tokens applying (in order)
filler removal, phrase replacements, formatting commands, or plain text
emission.

- Matching key strips punctuation — `src/text.rs:441-444`:

  ```rust
  fn spoken_key(word: &str) -> String {
      word.trim_matches(|character: char| character.is_ascii_punctuation())
          .to_ascii_lowercase()
  }
  ```

- Tokens keep `raw` and `key` — `src/text.rs:228-232` (`struct Token { raw: &'a str, key: String }`),
  built in `tokenize` at `src/text.rs:404-415`.
- Replacement consumption discards raw tokens — `src/text.rs:199-205`:

  ```rust
  if context.mode.applies_phrase_replacements()
      && let Some(replacement) = replacement_at(&phrase_replacements, &tokens, index)
  {
      output.push_text(&replacement.written);
      index += replacement.spoken.len();
      continue;
  }
  ```

- Command consumption likewise — `src/text.rs:207-213`, with the matchers
  `line_command_at` (`src/text.rs:356-364`) and `punctuation_command_at`
  (`src/text.rs:366-387`). All matching goes through `matches_phrase` /
  `matches_words` (`src/text.rs:425-439`), which compare `token.key` only —
  punctuation and case on the raw token are invisible to them.
- Filler removal — `filler_at` at `src/text.rs:389-402` (also key-based; for
  fillers, discarding attached punctuation is usually the desired behavior).
- Output assembly — `OutputText` at `src/text.rs:252-297`;
  `push_punctuation` trims trailing spaces then appends the mark.
- Tests at `src/text.rs:518-623`. The `format(input, context)` helper at
  `src/text.rs:522-527` is the structural pattern to follow for new tests.
  Note every existing input is lowercase/unpunctuated.

Conventions: plain functions over structs where possible, no new
dependencies, `just fmt` (nightly rustfmt with vertical item imports) before
finishing.

## Commands you will need

| Purpose   | Command                                     | Expected on success |
|-----------|---------------------------------------------|---------------------|
| Tests     | `just test text::`                          | all text tests pass |
| All tests | `just test`                                 | all pass (≥25 now)  |
| Check     | `just check`                                | exit 0              |
| Lint      | `cargo clippy --all-targets -- -D warnings` | exit 0              |

## Scope

**In scope** (the only file you should modify):
- `src/text.rs`

**Out of scope** (do NOT touch):
- `src/transcription.rs` — `RawTranscript` stays as-is; the fix belongs in
  the formatter, not the ASR boundary.
- `src/daemon.rs` — call sites don't change.
- Adding new spoken commands, modes, or LLM stages — separate work.

## Steps

### Step 1: Give tokens punctuation awareness

Extend `Token` so each token knows the punctuation attached to its raw form —
e.g. accessors/fields for the leading punctuation, the core word, and the
trailing punctuation (derived once in `tokenize`; `raw` may stay for plain
text emission). The exact representation is yours; what must be true:

- `matches_phrase`/`matches_words` (or a successor) can ask whether a
  multi-token span crosses a punctuation boundary.
- Replacement emission can re-attach a consumed span's outer punctuation.

**Verify**: `just check` → exit 0 (existing behavior still compiles; tests
still pass: `just test text::`).

### Step 2: Stop multi-token matches from crossing punctuation boundaries

A phrase or command spanning more than one token must NOT match if any
**non-final** token in the span carries trailing punctuation, or any
**non-first** token carries leading punctuation. ("question." + "Mark" is not
the command `question mark`; "new." + "Paragraph" is not `new paragraph`.)
Single-token matches are unaffected. Apply the same rule to phrase
replacements, line commands, punctuation commands, and multi-token fillers
("you know" — "you, know" should not match).

**Verify**: the Step 4 boundary tests pass: `just test text::` → pass.

### Step 3: Preserve attached punctuation through replacements and commands

What must be true:

- **Phrase replacements / dictionary terms**: leading punctuation of the
  first consumed token and trailing punctuation of the last consumed token
  are re-attached around the written form. "Gpui," → "GPUI," and "gee pee
  you eye." → "GPUI."
- **Punctuation commands**: the emitted mark replaces the span; any trailing
  punctuation on the final consumed token is dropped (saying "comma" where
  the ASR wrote "comma," must yield one comma, not two).
- **Line-break commands**: trailing punctuation on the consumed span is
  dropped (the ASR often renders a spoken "New paragraph." as its own
  sentence; the period must not survive onto the previous line).
- **Fillers**: current behavior (discard the token and its punctuation) is
  acceptable and should be locked in by a test, with one exception worth
  handling if cheap: a filler token whose trailing punctuation ends a
  sentence ("Um.") at minimum must not break sentence capitalization of what
  follows. If this exception turns into real design work, leave current
  behavior and note it in Maintenance notes instead.

**Verify**: `just test text::` → all pass, including Step 4's tests.

### Step 4: Golden tests for realistic ASR input

Add to the existing `tests` module in `src/text.rs`, using the `format`
helper (`src/text.rs:522-527`) as the pattern. Required cases, with exact
expected output:

| # | Mode / context | Input | Expected |
|---|----------------|-------|----------|
| 1 | Technical | `"I prefer GPUI."` | `"I prefer GPUI."` |
| 2 | Technical | `"Gpui, sherpa onnx, and Wayland."` | `"GPUI, sherpa-onnx, and Wayland."` |
| 3 | Message | `"That's a good question. Mark will answer."` | `"That's a good question. Mark will answer."` |
| 4 | Message | `"Something new. Paragraph two is next."` | `"Something new. Paragraph two is next."` |
| 5 | Message | `"Hello comma, world period."` | `"Hello, world."` |
| 6 | Email + dictionary `("gee pee you eye", "GPUI")` | `"I use gee pee you eye."` | `"I use GPUI."` |
| 7 | Message | `"Um, hello world."` | `"Hello world."` |
| 8 | Email | `"Hello comma New paragraph. Thanks period"` | `"Hello,\n\nThanks."` |
| 9 | Message | `"You, know the answer."` | `"You, know the answer."` |

If an expected output above conflicts with behavior you believe is more
correct once implementing, that is a design fork — STOP condition, not a
silent test edit. All nine existing formatter tests must keep passing
unchanged.

**Verify**: `just test` → all pass (34+ tests).

## Test plan

Covered by Step 4 (the golden table is the test plan). Pattern exemplar:
`src/text.rs:540-548` (`message_mode_applies_safe_spoken_punctuation`).

**Verify**: `just test` → all pass; `cargo clippy --all-targets -- -D warnings`
→ exit 0.

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `just test` → all pass, including 9 new golden tests
- [ ] `just check` → exit 0
- [ ] `cargo clippy --all-targets -- -D warnings` → exit 0
- [ ] Only `src/text.rs` modified (`jj st`)

## STOP conditions

Stop if:

- The code at the "Current state" locations doesn't match the excerpts.
- Any existing test's expected output must change to make the new behavior
  work — that is a behavior contract question for the maintainer.
- A golden case's required output seems wrong as you implement (e.g. the
  double-punctuation rule in case 5 conflicts with another case) — describe
  the conflict, don't pick.
- The fix appears to require changing `RawTranscript` or the tokenizer's
  public contract beyond `src/text.rs`.

On stopping, write a **handback**: current state, desired outcome, lingering
questions. Descriptive, not prescriptive.

## Maintenance notes

- The boundary rule (non-final trailing punctuation blocks a span) is the
  load-bearing decision; reviewers should probe it with quoted/parenthesized
  input ("(new paragraph)") and hyphenated terms.
- `spoken_key` trims *all* ASCII punctuation including apostrophes at word
  edges; contractions keep interior apostrophes and are unaffected.
- Deferred: smarter handling of a sentence-final filler ("Um." — see Step 3);
  capitalization after abbreviations ("e.g. test" capitalizes "Test") is a
  known, accepted limitation of `capitalize_sentences`.
