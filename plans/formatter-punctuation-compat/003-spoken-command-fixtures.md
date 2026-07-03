# 003 — Spoken-command fixture clips and real-audio characterization

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

**Source item:** `.agents/ROADMAP.md` System Upgrades row "Spoken-punctuation fixture clips"
**Effort index:** `plans/formatter-punctuation-compat/README.md`
**Planned at:** 2026-07-03, working copy `rvskvrqq` / git `1697863e`
**Depends on:** 001 (capture CLI), 002 (fix — otherwise these tests would lock in broken behavior), **and Josh recording clips (human input gate)**
**Executor target:** routine execution ready — no; blocked on human recording + license choice
**Source type:** roadmap (standing policy)
**Audit category:** tests
**Standards concern:** verification — the collision class that was found live and late must be caught by committed, agent-runnable checks
**Impact:** command-word speech joins the permanent corpus; 002's inferred characterization inputs are replaced with captured truth; the plan 004 Step 4 re-run gets a repeatable, headless equivalent
**Effort:** S (agent side) + one short recording session (Josh)
**Risk:** LOW — additive fixtures and tests; main uncertainty is the corpus WER budget under the current Whisper default
**Confidence:** HIGH
**Source direction:** "Self-recorded 16kHz command-word clips under `tests/fixtures/` (rules in `tests/fixtures/README.md` already fit), snapshot-guarded through the real formatter"

## Purpose

Public corpora are read prose — no public clip says "comma" or "new
paragraph" aloud, so no committed fixture can exercise the formatter×model
collision. The plan 004 handback exists only because a human dictated live
in Step 4 of an eval. Self-recorded command-word clips close that hole
permanently, and captured raw transcripts turn 002's inferred
characterization inputs into ground truth.

## What Better Means

- `tests/fixtures/` contains at least two spoken-command clips passing the
  corpus gate (`just test-integration`) under the current default model.
- `src/text.rs` characterization tests run on **captured** raw transcripts
  (Whisper now; Parakeet noted for the 004 re-run), not inferred strings.
- Regression = the corpus gate or formatter snapshots weakening to admit
  doubled punctuation.

## Current-State Evidence

- `tests/fixtures/README.md` — fixture contract: 16 kHz mono WAV, sibling
  `.txt` transcript, `LICENSE` per corpus dir, provenance in
  `manifest.toml`, checksums in `manifest.lock`, no unclear-provenance or
  generated clips; "Adding a fixture" checklist at the bottom is the
  authoritative procedure.
- `tests/fixtures/manifest.toml` — existing `[sources.<name>]` +
  `[[fixtures]]` schema to extend (includes `source_format` /
  `fixture_transform` fields for converted audio).
- `tests/integration.rs:56-122` — the corpus gate: aggregate WER ≤ 8%,
  CER ≤ 3%, per-fixture insta snapshot of the raw hypothesis;
  `normalize_for_asr_score` (`:266-283`) strips punctuation and case, so
  natively-punctuated hypotheses score cleanly against plain-word
  references.
- `plans/product-direction/004-default-model-parakeet.md` Step 4 + handback
  — the dictation script whose behavior must be representable:
  `Hello comma world period new paragraph thanks period. I use GPUI and sherpa onnx on Wayland.`
- Known Whisper misses on that script (handback eval table): `whalen`,
  `GPUi`, `Sherpa Onyx`, `JJ to scribe` — relevant to the WER budget below.

## Desired End State

- New corpus dir, e.g. `tests/fixtures/spoken-commands/`, with `LICENSE`
  (Josh's own recordings, license of his choosing — CC0 recommended),
  2–4 clips + sibling transcripts, manifest + lock entries.
- Whisper hypothesis snapshots committed via `just test-integration`.
- `src/text.rs`: 002's characterization test input replaced by (or joined
  with) the captured raw transcript, clearly attributed to its clip.
- A recorded note (in the manifest or fixture README) of the captured
  **Parakeet** raw transcripts for the same clips, ready for the 004 re-run
  — capturing them requires the ~600MB model download, so it is best-effort
  in the same session and mandatory during the 004 re-run.

## Scope

- New files under `tests/fixtures/spoken-commands/`;
  `tests/fixtures/manifest.toml`, `manifest.lock`, fixture README corpus
  section.
- `src/text.rs` — swap characterization inputs to captured strings.

## Out of Scope

- Long-form (>30s) fixtures — roadmap-deferred until the Parakeet re-run
  (they break the corpus gate under the Whisper default).
- Flipping the default model; re-running plan 004.
- Any formatter rule change — if captured audio shows the rules are wrong,
  that is a STOP back to 002, not an improvised amendment here.

## Design Claim

Verification (`coding-standards` → `verification.md`): checks must cover
the failure class where it actually lives — command-word prosody through a
real recognizer — not only synthetic strings; and fixture provenance rules
exist so the corpus stays redistributable.

## Architecture Diagnosis

N/A — test-asset plan.

## Recording Brief (for Josh — human input gate)

- 2–4 clips, each ≤ 20 seconds (stay clear of the 30s Whisper ceiling),
  quiet room, normal dictation cadence.
- Clip 1 (mandatory): the plan 004 Step 4 script verbatim —
  "Hello comma world period new paragraph thanks period. I use GPUI and
  sherpa onnx on Wayland."
- Clip 2 (mandatory): punctuation-command-dense —
  e.g. "is this working question mark yes exclamation mark item one colon
  audio semicolon item two comma text period"
- Clip 3 (recommended): command words as content —
  e.g. "that is a good question. mark will answer. the sentence has a comma
  in it." (guards `span_allows_match` behavior on real audio)
- Any capture format is fine; the conversion command is in the fixture
  README: `ffmpeg -y -i <src> -ac 1 -ar 16000 -sample_fmt s16 <out>.wav`
- Decide the license for your own recordings (CC0 recommended; any
  redistribution-clean choice works).

## Implementation Sequence

### Step 1 — Receive and convert recordings

Convert to 16 kHz mono s16 WAV per the README command; record the exact
conversion command as `fixture_transform` in `manifest.toml`. Write sibling
`.txt` transcripts as the **verbatim spoken words** (command words as words:
"hello comma world period …") — `normalize_for_asr_score` makes these score
correctly against both plain and natively-punctuated hypotheses.

### Step 2 — Manifest, lock, license

Follow `tests/fixtures/README.md` "Adding a fixture" steps 1–5: corpus dir
with `LICENSE`, `[sources.spoken-commands]` block (source: self-recorded,
recorded-by, date, license), `[[fixtures]]` entries, checksums into
`manifest.lock`. Add a short "Spoken commands" section to the fixture
README's "Current corpora".

### Step 3 — Corpus gate

`just test` (fixture discovery + transcript presence), then
`just test-integration`: review and commit the new per-fixture snapshots;
confirm aggregate WER/CER stay under thresholds. See the WER-budget STOP
below if they do not.

### Step 4 — Capture raw transcripts and re-seed characterization

For each clip: `just run transcribe <clip> --raw` (default model), and
if the Parakeet model is available,
`just run transcribe <clip> --raw --model parakeet-tdt-0.6b-v2-int8`.
Replace 002's inferred input string in
`parakeet_native_punctuation_coexists_with_spoken_commands` with the
captured Parakeet raw transcript (or, if Parakeet is unavailable this
session, add the captured Whisper string as a second characterization case
and leave the inferred one marked as such). Confirm formatted output is
clean; update snapshots deliberately.

### Step 5 — Record the Parakeet transcripts for the 004 re-run

Wherever captured, note the Parakeet raw strings in the manifest comments
or fixture README so the 004 re-run's Step 4 can diff live behavior against
them.

## Verification

### Automated

- [ ] `just test` — fixture discovery passes; formatter tests green on
  captured inputs.
- [ ] `just test-integration` — corpus (including new clips) under
  WER 8% / CER 3%; new snapshots committed.
- [ ] `just check`, `just clippy`, `just fmt` — clean.

### Evals / Regression Checks

- [ ] The new fixtures under the corpus gate are themselves the standing
  eval for this failure class.
- [ ] Characterization tests in `src/text.rs` now assert against captured
  model output — inference risk retired.

### Manual

- [ ] Josh reviews the committed `LICENSE` text once (his recordings, his
  terms) — nothing else.

## Autonomy Boundary

Routine execution may include:

- Steps 1–5 in full once recordings exist, including snapshot review of
  raw-hypothesis snapshots (they are descriptive, not judged).

Design review is required for:

- Any change to 002's rule table prompted by captured audio (goes back
  through 002's review boundary).

Human approval is required for:

- The recordings themselves and their license (Josh);
- Downloading the ~600MB Parakeet model in a metered/CI environment.

## Drift Checks

Before editing, the executor must:

- [ ] Re-read this plan, the effort index, and
  `tests/fixtures/README.md` (the authoritative add-a-fixture procedure —
  it wins over this plan on procedure details).
- [ ] Confirm 001 and 002 have landed (`jj log`; `dictate transcribe`
  exists; `src/text.rs` has the dedup rules).
- [ ] Re-check `manifest.toml` schema against the newest committed entries.

## STOP Conditions

Stop and hand back if:

- **Command words rendered as symbols:** a model transcribes spoken
  "question mark" as a literal `?` (or "comma" as `,`) instead of the
  words. `normalize_for_asr_score` (`tests/integration.rs:266-283`) strips
  the symbol from the hypothesis while the reference still contains the
  words, so WER breaks — and worse, the formatter never sees a command word
  to match. Inspect each raw snapshot for this **before** committing
  transcripts; if it occurs, hand back with the affected clip and model —
  the resolution (reference wording, per-model expectations, or accepting
  the model's symbol rendering as the desired behavior) is a design
  decision, not an executor call;
- **WER budget:** adding the clips pushes aggregate WER over 8% (plausible:
  Whisper misses `wayland`/`jj describe`-class terms; the handback shows
  four known misses on clip 1's vocabulary). Handback options for the
  maintainer: reword clips to avoid known-miss vocabulary, or wait for the
  Parakeet flip — do **not** raise thresholds or drop the corpus gate;
- captured Parakeet raw output contradicts 002's rule table (e.g. R4's
  keep-punctuation-before-line-breaks is wrong in practice) — back to 002's
  design review;
- a clip cannot be made to satisfy the fixture contract (format, license,
  provenance);
- `dictate transcribe` is missing or its output streams changed.

## Rejected Approaches

- Synthesized TTS command clips — fixture rules ban generated/unclear
  provenance; unrepresentative prosody (effort index).
- Reusing the daemon + mic for capture — exactly the human-in-the-loop
  dependency this effort exists to remove; the CLI path is the loop.
- Putting formatter expectations in integration snapshots — the fixture
  README draws the line: raw hypotheses live in integration snapshots,
  formatter behavior lives in `src/text.rs` snapshots. Keep it.

## Standing Policy Updates

This plan discharges the roadmap's "Spoken-punctuation fixture clips"
standing-policy row. Future model-behavior claims should add a fixture
here rather than relying on live dictation.

## Executor Notes

- The `.txt` transcript is the *spoken words*, not the expected formatted
  output — formatted expectations belong in `src/text.rs` tests.
- Keep clips short; a >30s clip silently truncates under the current
  default and torpedoes WER (the roadmap's long-form-fixture ordering trap).
- `manifest.lock` wants checksums of both source and committed files —
  follow the existing entries' format exactly.
- When updating 002's characterization input, keep the test name and intent;
  only the input string and (if needed) snapshot change.
- Never edit WER/CER thresholds in `tests/integration.rs` — that is a
  STOP, not a knob.
