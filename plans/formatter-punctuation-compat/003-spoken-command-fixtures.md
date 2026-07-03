# 003 — Spoken-command fixture clips and real-audio characterization

> **Executor instructions:** Follow this plan with no hidden session context. You can assume the executor is competent at explicit instructions and weak at filling gaps, resolving ambiguity, or knowing when to stop. If a STOP condition occurs, write a handback instead of improvising.

> **Revised 2026-07-03 (v2, TTS-first):** originally gated on Josh
> self-recording clips. A live spike (Piper TTS → `dictate transcribe`)
> proved synthesized speech reproduces the collision class on real
> Parakeet output and validated the 002 fix end-to-end, so TTS generation
> is now the primary path and human recording is optional enrichment.
> Spike evidence is inlined below.

**Source item:** `.agents/ROADMAP.md` System Upgrades row "Spoken-punctuation fixture clips"
**Effort index:** `plans/formatter-punctuation-compat/README.md`
**Planned at:** 2026-07-03, working copy `rvskvrqq` / git `1697863e`; revised same day after 001+002 landed (`7e27ed98`, `9e564ef5`)
**Depends on:** 001 (capture CLI, DONE), 002 (fix, DONE), and **ordering: land the committed fixtures with or after the Parakeet default flip (plan 004 re-run)** — see the Whisper-WER STOP below
**Executor target:** routine execution ready — yes for clip generation and transcript capture; the fixture *commit* rides with the 004 re-run
**Source type:** roadmap (standing policy)
**Audit category:** tests
**Standards concern:** verification — the collision class that was found live and late must be caught by committed, agent-runnable checks
**Impact:** command-word speech joins the permanent corpus; 002's inferred characterization inputs are replaced with captured truth; the plan 004 Step 4 re-run gets a repeatable, headless equivalent
**Effort:** S — fully agent-executable
**Risk:** LOW — additive fixtures and tests; main uncertainty is per-model WER on the synthetic voice (measured for whisper-base-en and parakeet v2 in the spike)
**Confidence:** HIGH — the pipeline has already been run end-to-end once
**Source direction:** "Self-recorded 16kHz command-word clips under `tests/fixtures/` (rules in `tests/fixtures/README.md` already fit), snapshot-guarded through the real formatter" — amended to TTS-generated with recorded provenance

## Purpose

Public corpora are read prose — no public clip says "comma" or "new
paragraph" aloud, so no committed fixture can exercise the formatter×model
collision. The plan 004 handback exists only because a human dictated live
in Step 4 of an eval. TTS-generated command-word clips (spike-proven to
trigger the collision on real Parakeet output) close that hole permanently,
and captured raw transcripts turn 002's inferred characterization inputs
into ground truth.

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
  (CC0 dedication + Piper `en_US-ljspeech` provenance note), 2–3 generated
  clips + sibling transcripts, manifest + lock entries recording the exact
  generation commands.
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

## Spike Evidence (2026-07-03, do not re-derive)

Piper TTS (`en_US-lessac-medium`, local, via `uvx --from piper-tts piper`)
speaking three scripts, converted to 16 kHz mono s16, run through
`dictate transcribe --raw` per model. Findings that shape this plan:

- **The collision reproduces on real Parakeet output from TTS audio.**
  Punctuation-dense script → Parakeet raw:
  `Is this working question mark? Yes, exclamation mark item 1 colon audio, semicolon item 2, comma text period.`
  (native `?` attached after the spoken words "question mark"; `,` attached
  before "exclamation mark"). The 002 formatter produced
  `Is this working? Yes! Item 1: audio; item 2, text.` — fix validated on
  real model output.
- **Flat prosody under-triggers the collision.** The plan-004 script read
  as a plain word stream got *no* attached punctuation from Parakeet
  (raw: `Hello comma world period new paragraph thanks period …`) and
  formatted perfectly. Scripts must be punctuation-dense (the model needs
  pause-shaped context) for the collision fixture to earn its keep; a
  flat-prosody clip is still useful as a plain command-transcription
  fixture.
- **Content-word hazard demonstrated.** With no model-emitted boundary
  punctuation, `that is a good question. mark will answer …` transcribed
  without the `.` and formatted to `That is a good?` — the pre-existing
  single-word/two-word command false positive (out of scope here; roadmap
  tracks it). Do not commit a fixture that asserts this broken output as
  desired; if a content-word clip is committed, its formatter expectation
  documents current behavior with a comment naming the hazard.
- **Whisper-base mangles the synthetic voice on some scripts**
  (`Hello, Kuma World`, `Welland`) — a committed TTS fixture can blow the
  corpus WER gate under the *current* default. Parakeet transcribed the
  same clips near-verbatim. Hence the ordering dependency: commit these
  fixtures with/after the Parakeet flip.
- **Both models ITN numbers** ("item one" → `item 1`), which breaks
  word-reference WER scoring. Scripts must avoid cardinals/ordinals.
- **TTS mispronounces exotic technical terms** ("onnx" → "onks") — keep
  `GPUI`/`sherpa onnx`/`Wayland` out of TTS scripts; those belong to
  optional human-recorded clips where the maintainer's real pronunciation
  is the point.

## Clip Production (TTS-first; agent-executable)

- Voice: **`en_US-ljspeech`** (medium or high) from `rhasspy/piper-voices`
  — its training dataset (LJ Speech) is public domain, matching the
  provenance bar of the existing `tests/fixtures/ljspeech/` corpus. Do NOT
  use `lessac` (Blizzard 2013 research license) or `hfc_female`
  (CC BY-NC-SA) for committed fixtures; the spike used lessac for
  local-only capture, which is fine.
- Generation shape (record the exact commands as `fixture_transform`):
  `printf '<script>' | piper -m en_US-ljspeech-medium.onnx -f raw.wav`
  then `ffmpeg -y -i raw.wav -ac 1 -ar 16000 -sample_fmt s16 <fixture>.wav`
- **Non-determinism (measured 2026-07-03):** Piper synthesis is not
  byte-stable — re-running the identical command produced a nonviable
  variant (`Aloha World …`). The retained WAVs are the source of truth;
  generation commands are provenance documentation only. **Executed:** the
  captured finals live at `plans/formatter-punctuation-compat/clips/` with
  checksums in `003-capture-notes.md`; the fixture-commit step moves them
  into the corpus rather than regenerating.
- Scripts (each ≤ 20 s; no numbers, no exotic technical terms):
  - Clip A (mandatory, collision-dense): punctuation commands packed
    tightly so the model emits native marks around them — spike-proven
    shape: "is this working question mark yes exclamation mark item colon
    audio semicolon next item comma text period"
  - Clip B (mandatory, plain command stream): the plan-004 script minus
    technical terms — "hello comma world period new paragraph thanks
    period" plus a short plain-prose sentence.
  - Clip C (optional, content words): "that is a good question. mark will
    answer. the sentence has a comma in it." — commits the false-positive
    hazard as documented current behavior.
- Sibling `.txt` transcripts: the verbatim spoken words.
- **Optional human enrichment (not a gate):** Josh may later record the
  same scripts (plus technical-vocabulary lines) for realistic prosody;
  those replace or join the TTS clips under his own license (CC0
  recommended).

## Implementation Sequence

### Step 1 — Generate and convert clips

Download the `en_US-ljspeech` voice (model + config JSON) from
`rhasspy/piper-voices`, generate the scripts from the Clip Production
section, convert to 16 kHz mono s16 WAV per the README command, and record
the exact generation + conversion commands as `fixture_transform` in
`manifest.toml`. Write sibling `.txt` transcripts as the **verbatim spoken
words** (command words as words: "hello comma world period …") —
`normalize_for_asr_score` makes these score correctly against both plain
and natively-punctuated hypotheses.

### Step 2 — Manifest, lock, license

Follow `tests/fixtures/README.md` "Adding a fixture" steps 1–5: corpus dir
(e.g. `tests/fixtures/spoken-commands/`) with `LICENSE` (CC0 dedication for
the generated audio, plus a provenance note: Piper `en_US-ljspeech` voice,
trained on the public-domain LJ Speech dataset, model card URL and
revision), `[sources.spoken-commands]` block (generator, voice, voice
model checksum, scripts), `[[fixtures]]` entries, checksums into
`manifest.lock`. Add a short "Spoken commands (synthesized)" section to the
fixture README's "Current corpora" — including one sentence acknowledging
these are self-generated TTS with known source, which the fixture rules'
"generated-by-unknown-source" ban does not cover.

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

- [ ] Josh approves committing self-generated TTS fixtures (the
  known-source reading of the fixture rules) and skims the `LICENSE` +
  provenance note once — nothing else.

## Autonomy Boundary

Routine execution may include:

- Steps 1–5 in full, including clip generation and snapshot review of
  raw-hypothesis snapshots (they are descriptive, not judged).

Design review is required for:

- Any change to 002's rule table prompted by captured audio (goes back
  through 002's review boundary);
- Script wording changes beyond the Clip Production section (they encode
  spike-derived constraints: no numbers, no exotic technical terms,
  punctuation-dense for the collision clip).

Human approval is required for:

- Committing the fixtures (Josh signs off on the TTS-with-known-source
  reading of the fixture rules — flagged, recommended, not yet ruled on);
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
- **WER budget / ordering:** the corpus gate runs under the default model.
  The spike measured whisper-base-en badly mistranscribing the synthetic
  voice on some scripts (`Kuma World`, `Welland`). **Update (2026-07-03
  capture run):** the final tuned clips measured whisper word-error counts
  of A=0, B=2, C=0 — likely within the aggregate 8% budget even under the
  whisper default, so the ordering gate is now *verify-at-commit-time*
  rather than assumed-blocking: run `just test-integration` with the
  fixtures in place and only defer to the Parakeet flip if the gate
  actually fails. Do **not** raise thresholds or drop the corpus gate;
  transcript capture and formatter characterization (Steps 4–5, executed
  2026-07-03) pinned the model id explicitly and did not need to wait;
- captured Parakeet raw output contradicts 002's rule table (e.g. R4's
  keep-punctuation-before-line-breaks is wrong in practice) — back to 002's
  design review;
- a clip cannot be made to satisfy the fixture contract (format, license,
  provenance);
- `dictate transcribe` is missing or its output streams changed.

## Rejected Approaches

- Human recording as a *gate* (this plan's v1) — reversed by the spike:
  TTS reproduces the collision on real Parakeet output, is reproducible
  from commands recorded in the manifest, and removes the human
  dependency. Human clips remain welcome enrichment (realistic prosody,
  technical vocabulary) but nothing blocks on them. The original
  rejection of TTS conflated "generated-by-unknown-source" (banned) with
  self-generated-with-recorded-provenance (not banned) and overweighted
  the prosody concern, which the spike measured instead of assumed.
- YouTube-ripped audio for committed fixtures — standard-license YouTube
  content is not redistributable, and even CC-BY uploads sit behind ToS
  friction plus per-video license verification; strictly worse than
  public-domain-voice TTS for this corpus. (Local-only, non-committed use
  for ad-hoc capture is the maintainer's personal call and needs no plan.)
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
