# Research: automatic formatting parity + a GPUI debug harness

Date: 2026-07-03. Method: deep-research workflow (5 search angles → 15
sources fetched → falsifiable claims extracted → 3-vote adversarial
verification per claim, ≥2/3 refutations kill; 103 agents total), plus
direct local reads of the dependency sources on disk (`sherpa-onnx` 1.13.2
crate in the cargo registry; the pinned zed checkout at rev `50d001f` in the
cargo git cache). Every finding below survived verification 3–0 or is a
local code read; killed and unverified claims are noted where relevant.

## Track 1 — Transcription formatting without spoken commands

### Headline

**"Get punctuation at all" is already solved for Dictate — twice over.**
The exact ask ("hello how are you" → "Hello, how are you?") is delivered
natively by the Parakeet default flip that's already the roadmap's critical
path, and — as a fallback for uncased models — by punctuation-restoration
APIs that ship in the `sherpa-onnx` crate Dictate already links. The open
frontier, where the premium tools differentiate, is *cleanup beyond
punctuation* (filler-aware rephrasing, phonetic vocabulary correction,
self-correction handling) — and every tool that ships it does so with an
LLM. No surveyed tool bundles a local cleanup model.

### Native ASR punctuation (the default path)

NVIDIA's model cards for parakeet-tdt-0.6b **v2 and v3 both** list
"Automatic punctuation and capitalization" as a key feature (a claim that
only the special "Unified" variant does this was refuted 3–0 against the
primary model cards). VoiceInk — the closest open-source comparator — ships
Parakeet TDT v3 as its recommended fully-offline model. Dictate's planned v2
default (English, better English WER) is at or ahead of field baseline.

### Punctuation restoration inside sherpa-onnx (the fallback path)

Verified against docs, the release artifacts, and the local crate source:

| Model | Size | Does | Doesn't | Latency (doc examples) |
|---|---|---|---|---|
| `sherpa-onnx-online-punct-en-2024-08-06` (CNN-BiLSTM, Edge-Punct-Casing, arXiv:2407.13142) | 28 MB fp32 / **7.1 MB int8** | English punctuation **and casing** ("how are you i am fine thank you" → "How are you? I am fine. Thank you.") | proper-noun casing quality unmeasured | ~13–30 ms |
| CT-Transformer zh-en (FunASR, offline) | 281 MB fp32 / ~65 MB int8 | zh+en punctuation | **no casing**; can emit Chinese full-width marks on pure English (issue #2568) | ~3–14 ms |

The Rust crate at the pinned version (1.13.2, read locally) exposes both:
`OfflinePunctuation::create(&config)` / `OnlinePunctuation::create(&config)`
with `add_punctuation(&str)` — the doc example is literally
`add_punctuation("today is a good day how are you")`. Wiring this is one
config struct plus a model-catalog entry; **zero new runtimes**.

Caveats: the punctuation-model inventory is frozen (three archives, newest
Aug 2024 — usable but stagnant); latency figures are single doc-example runs
on unspecified hardware; whether the online-punct model *degrades* already-
punctuated Parakeet output if run unconditionally is unmeasured — if wired,
gate it to uncased-output models only.

### The industry pipeline shape (verified from source)

VoiceInk's pipeline (repo read at commit `ba0d7c7d`, same-day):
`transcribe → filter → format → word-replace → AI enhance (optional, last) → deliver`.
The deterministic pre-LLM stages:

- **Filler removal**: regex `\b<word>\b[,.]?` over a user-editable default
  list (`uh, um, uhm, umm, uhh, hmm, hm, mmm, mm, mh, ehh…`) — note the
  optional trailing punctuation eaten with the filler, which matters once
  the ASR natively punctuates ("um, so" → "so", not ", so").
- **Word replacements**: lookaround regex, longest-first, case-insensitive
  (Dictate's `text.rs` already sorts longest-first — parity).
- **Paragraph chunking**: sentence-tokenized, ~50-word target, max 4
  sentences per paragraph — long dictation arrives pre-paragraphed.
- **Hallucination stripping**: bracketed ASR artifacts filtered
  (Dictate's `transcript_is_noise` covers whole-utterance junk but not
  inline `(cough)` mid-transcript).

macparakeet (v0.6.24) ships the same shape: sub-1ms deterministic pipeline
(filler removal, replacements, snippet expansion, whitespace/punctuation-
spacing cleanup, first-letter capitalization), "no AI involved" in the core
dictation path; AI features entirely opt-in.

**Dictate's deterministic pipeline is already at parity with this layer.**
The borrowable increments are the punctuation-aware filler regex, inline
bracket-artifact stripping, and paragraph chunking.

### The LLM tier (idea source; deferred remains right)

- **Wispr Flow** (category-definer, cloud, closed): automatic edit pass —
  rambling → structured text, fillers/typos removed, real-time
  self-correction handling ("no wait, scratch that" applied, not
  transcribed). Reviews attribute it to a fine-tuned Llama-family model,
  cloud-side.
- **VoiceInk**: prompt-based chat-completion rewrite; seven cloud providers
  plus `ollama` (localhost:11434) and a generic local-CLI template. Modes:
  Polish/Email/Chat/Post. **No bundled local model.**
- **Vocabulary handling at the LLM tier is prompt-layer, not string
  matching**: VoiceInk embeds the user vocabulary in the system prompt as
  "the spelling authority" with an explicit anti-over-replacement hedge
  ("Do not force a replacement when the text clearly means something
  else") — this is what fixes *phonetically close* mistakes
  (`Sherpa Onyx` → `sherpa-onnx`), which deterministic replacement
  fundamentally cannot. Wispr **auto-learns** the dictionary from user
  corrections (proper nouns, synced) instead of manual entry.
- **Self-correction and number/date/email formatting**: undocumented even
  in the closest open-source comparators — genuinely unsolved outside the
  LLM stage, not table stakes Dictate is missing.
- **No field-proven local model-size floor exists** — every shipping tool
  punts to external LLMs, so committing to a bundled local cleanup model
  would be charting new territory; needs an empirical spike (1–4B instruct
  on target CPU) before any plan.

### Track 1 ranked borrow-list (mapped to Dictate's seams)

1. **Land the Parakeet default** (roadmap Now #1 → Next). Native
   punctuation *is* the user's ask. No new work beyond what's planned.
2. **Deterministic pipeline increments** in `src/text.rs`:
   punctuation-aware filler regex, inline bracketed-artifact stripping,
   sentence-tokenized ~50-word paragraph chunking. Regex/string-level,
   LLM-free, snapshot-testable with the existing insta setup.
3. **Optional `OnlinePunctuation` fallback** via the existing crate +
   model catalog, gated to models whose output is uncased — a
   "punctuation: native | restore | none" capability on catalog entries
   (dovetails with the roadmap's "model punctuation capability flag"
   standing-policy row). Only worth it if non-Parakeet models stay
   user-selectable; skip if the catalog trims to punctuated models.
4. **Auto-learned dictionary** (Wispr's trick): requires history +
   a correction signal; design after history/database lands. Later.
5. **Optional, off-by-default local LLM cleanup stage** behind a trait
   (VoiceInk's prompt shape: vocabulary-as-spelling-authority + hedge),
   local backends only. Keep in Later/deferred; precede with the
   model-size spike. This is the only route to Wispr-parity cleanup and
   also the only item with real design risk.

**Open question the field could not answer** (no surviving evidence, and
initial-pass sources contained refuted errors in exactly this area): how to
reconcile native ASR punctuation with spoken punctuation commands — no
surveyed tool documents keeping both. Dictate's formatter-compatibility fix
is charting its own path; the roadmap's design-review flag on the dedup
rules is justified. The pragmatic prior from the survey: tools with
natively-punctuating models simply *drop* the spoken-command layer or leave
it to modes — worth considering "spoken commands off by default under
Parakeet" as a design option in that plan.

## Track 2 — A `dictate debug` harness (game-dev-style)

### What Zed actually does now

- `crates/storybook` and `crates/story` were **deleted** 2026-04-09
  (merged PR #53511: "component previews are now handled by the
  component_preview crate"). Copying storybook is dead — including at
  Dictate's pinned rev.
- At the pinned rev `50d001f` (verified 404 for storybook, both crates
  present; also read locally from the cargo git cache):
  - **`crates/component`** — a small `Component` trait: `id`, `name`,
    `description`, `scope` (enum), `status`, and
    `preview(&mut Window, &mut App) -> AnyElement`. Registration via
    `inventory` distributed slices (`inventory::submit!` +
    `ComponentFn::new(register_fn)`; `component::init()` drains the
    inventory into a `LazyLock<RwLock<ComponentRegistry>>`). Deps: gpui,
    inventory, parking_lot, strum, collections, theme — **the pattern is
    ~200 lines and portable**.
  - **`crates/component_preview`** — the in-app gallery. Deps include
    `workspace`, `project`, `client`, `db`, `settings`, `ui` — **deeply
    Zed-bound, not extractable**. Copy its *ideas* (scoped component list,
    per-component example groups via `ComponentExample { variant_name,
    description, element }`), not its code.
- `gpui::TestAppContext` and `VisualTestContext` both exist at the pinned
  rev (`crates/gpui/src/app/test_context.rs`, `visual_test_context.rs`) —
  headless view testing is available. What exactly renders headless vs
  needs a real window was **not** settled by the research (claims didn't
  survive verification); treat as a question to answer empirically during
  planning.

(Track 2 caveat: research coverage beyond the Zed-crate facts was thin —
longbridge/gpui-component gallery patterns and game-dev-tooling claims
didn't survive verification. The recommendation below rests on the verified
Zed pattern plus Dictate's own seams.)

### Recommended architecture direction (not a final design)

A `dictate debug` subcommand opening a **normal window** (not layer-shell)
— GPUI is already resident via `app.rs`, so this is a second window kind,
no daemon, no socket:

- **Component gallery**: a Dictate-local `DebugComponent` trait modeled on
  Zed's `Component` (name/description/preview-fn). With two components
  today, a **static registry list beats inventory** — adopt distributed
  registration only when the component count makes the central list
  annoying. Tabs (or a left list, like component_preview) per component.
- **Scenario cycling**: a scenario enum per component — for the overlay:
  `Idle / Recording(fake spectrum) / Transcribing / Error(message)` —
  cycled by button/keystroke. This is exactly the game-dev "state selector"
  pattern, and it forces the phase states of product-direction plan 005 to
  exist as *renderable data*, which is the real architectural win: the
  overlay view stops being drivable only by a live daemon.
- **Deterministic data injection**: synthetic `SpectrumLevels` generators
  (sine sweep, constant, recorded-frame playback) behind the same handle
  the daemon feeds today. The seam already exists (`overlay` handle +
  `SpectrumLevels`); the debug window is just a second producer.
- **In-window transcription bench**: file picker / fixture list →
  `audio::load_wav_utterance` → the public `transcription::transcribe`
  seam → `DictationFormatter` — showing **raw vs formatted side by side**,
  with timing. This reuses the exact seams the integration harness
  established (commits `471e771`/`10334b3`/`a66650e`) and turns the fixture
  corpus into an interactive eval bench — directly useful for the
  formatter-compatibility work, since a Parakeet-punctuated fixture can be
  eyeballed through the formatter without dictating live.
- **Debug info overlay**: per-frame stats the overlay already implicitly
  has (bands, gate state, fps) rendered as text in the debug window.
  Time controls (pause/step) are a later nicety; don't build them first.
- **Headless tests**: separately from the window, spike
  `gpui::TestAppContext` for `OverlayView`/`Waveform` render tests — if it
  works at the pinned rev, the waveform gating logic (roadmap Later item)
  gets real tests; if not, extracting `advance_waveform` as a pure function
  (already roadmapped) is the fallback.
- **Dual-use for agents (maintainer seed, added post-research)**: the
  harness must be agent-drivable, not just interactive — every scenario
  reachable by clicking must be reachable headless: CLI flags
  (`dictate debug --scenario recording --capture out.png --exit`),
  machine-readable output, meaningful exit codes. The same seed generalizes
  beyond the harness into an agentic-feedback-loop ladder: (1) a headless
  `dictate transcribe <wav> [--raw|--formatted]` subcommand (agents iterate
  on formatter changes against real model output, no mic, no human); (2)
  daemon audio injection (`record start --from-file x.wav`) so the full
  socket→phase→transcribe→deliver pipeline runs without hardware; (3) the
  socket ack protocol so agents assert daemon state from exit codes. All
  three are recorded in `.agents/ROADMAP.md` (Now #1 slice, Later, and the
  System Upgrades standing-policy row).

### Track 2 ranked borrow-list

1. **Zed's `Component`-trait shape** (name/description/scope/preview-fn) —
   copy the pattern into a `dictate debug` gallery; skip `inventory` until
   component count demands it.
2. **Scenario-enum state selector** (game-dev pattern, and
   component_preview's example-groups idea) — drives plan 005's phase
   states as data.
3. **Fake-producer injection through the existing overlay/spectrum seam** —
   no new abstraction needed, just a second feeder.
4. **WAV → transcribe → format bench in-window** — reuses the integration
   harness seams verbatim.
5. **`TestAppContext` spike** for headless view tests — answer empirically;
   unverified by research.

## How this maps onto `.agents/ROADMAP.md`

- Validates Now #1 (formatter fix) and adds a design option to consider
  there: spoken commands off-by-default under natively-punctuating models.
- Adds a **new Next-tier candidate**: the `dictate debug` harness — it
  materially de-risks plan 005 (overlay phase states) and the formatter
  work, and should probably be planned *before or with* 005.
- Strengthens the "model punctuation capability flag" standing-policy row
  (borrow-list item 3 gives it a concrete consumer).
- Track 1 items 2–3 are new small opportunities for the formatter/catalog;
  items 4–5 land in Later (item 5 was already deferred — now with evidence
  that nobody in the field has shipped the local version either).

## Killed / unverified claims (so they aren't re-researched)

- "Only macparakeet's Parakeet-Unified model has native punctuation;
  standard v2/v3 don't" — **refuted 3–0**: NVIDIA model cards for v2 and v3
  both list automatic punctuation + capitalization.
- "VoiceInk's cleanup is cloud-only" — **refuted**: ships Ollama + local-CLI
  options (though cloud-first in UX, default Gemini).
- Aqua Voice, Superwhisper, MacWhisper, Handy, Voxtype, OpenWhispr,
  hyprvoice, Speech Note: produced **no surviving claims** — the parity
  survey generalizes from Wispr Flow, VoiceInk, and macparakeet only.
- longbridge/gpui-component gallery internals, imgui-style Rust debug-panel
  ecosystem, TestAppContext capabilities: **no surviving claims** — open.

## Key sources

- sherpa-onnx punctuation: k2-fsa.github.io/sherpa/onnx/punctuation/
  (index + pretrained_models), github.com/k2-fsa/sherpa-onnx (issue #2568,
  punctuation-models release), docs.rs/sherpa-onnx/1.13.2, arXiv:2407.13142
- VoiceInk: github.com/beingpax/VoiceInk (TranscriptionPipeline.swift,
  Processing/, Services/AIEnhancement/), tryvoiceink.com/features + docs
- Wispr Flow: wisprflow.ai, docs.wisprflow.ai, five independent 2026 reviews
- macparakeet: github.com/moona3k/macparakeet (README v0.6.24)
- Zed: PR zed-industries/zed#53511; crates/component +
  crates/component_preview at rev 50d001fe0a38 (GitHub API + local cargo
  git cache read)
- NVIDIA: huggingface.co/nvidia/parakeet-tdt-0.6b-v2 and -v3 model cards
