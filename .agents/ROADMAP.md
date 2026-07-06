# Strategic Roadmap: dictate

## Purpose

For the maintainer (Josh) and future planning agents working in this single
repository. Horizon: the next few planning cycles of the GPUI-native rewrite.
Decision supported: which planning artifact to create next, without redoing
repo discovery. Generated 2026-07-03 at working copy `579d27c2` (parent
`fc9c3b87`, "Curate transcription fixture corpus").

## What Better Means

The repo's own thesis (`plans/product-direction/README.md`): the gap between
Dictate and the loved macOS dictation apps is, in order, **delivery** (text
lands where the user works), **configurability** (formatter + model catalog
reachable), **model quality** (Parakeet as local default), and **legible
overlay states**. To that this roadmap adds two repo-health criteria:
**failure honesty** (a resident daemon must degrade, not brick) and
**verification confidence** (the new model-backed corpus harness should guard
every claim the product makes about transcription quality).

## Source Material Reviewed

- `plans/gpui-rewrite-hardening/README.md` + plans 001–006 — all DONE; its
  rejected/deferred lists were honored (nothing re-reported here).
- `plans/product-direction/README.md` + plans 001–006 — 001–003 DONE, 004
  BLOCKED with a detailed handback, 005/006 TODO.
- `plans/product-direction/004-default-model-parakeet.md` — the handback with
  the full 3-model eval table and the formatter-compatibility STOP.
- `plans/product-direction/spike-insertion-findings.md` — decided insertion
  verdict, seam sketch, 4 open maintainer questions.
- `PLAN.md` — behavior-keep inventory (history, device UX, model UX, tray,
  timer) and rewrite constraints; parts now stale.
- `README.md`, `AGENTS.md`, `Justfile`, `Cargo.toml`, `.github/workflows/`
- Source: `src/daemon.rs`, `src/dictation.rs`, `src/settings.rs`,
  `src/delivery.rs`, `src/transcription.rs`, `src/models.rs`, `src/mic.rs`,
  `src/text.rs` (shape + tests), `tests/integration.rs`,
  `tests/fixtures/README.md` — all findings below verified by direct read.
- VCS state: local `main` (`e6ce38bb`) is ~17 commits ahead of `main@origin`
  (`692432f6`); 4 more unbookmarked commits above it (`471e771f`…`fc9c3b87`,
  the transcription eval harness); ~15 dead bookmarks.
- `.agents/research/formatting-parity-and-debug-harness.md` (2026-07-03,
  added after the initial roadmap pass) — verified parity survey of
  formatting pipelines (Wispr Flow, VoiceInk, macparakeet), sherpa-onnx
  punctuation-restoration options, and the Zed component/component_preview
  pattern for a `dictate debug` harness. Items below marked *(research)*
  trace to it.

## Audit Coverage / Not Reviewed

- **Reviewed deeply:** daemon/socket, dictation state machine, mic capture,
  delivery, settings, transcription seam, model catalog/download, formatter
  shape and test coverage, integration harness, CI, all plan artifacts.
- **Sampled lightly:** `src/overlay.rs`, `src/spectrum.rs`, `src/app.rs`,
  `src/components/`, `examples/` (insertion spike prototypes).
- **Not reviewed:** Tauri-era history, the pinned gpui tree itself (one
  comment-cited region excepted), `.scratch/`, `.private-journal/`.
- **Audit categories covered:** correctness, security (bounded — prior
  audit's rejected security items honored, no new pass beyond socket/daemon
  paths), performance (light), tests, architecture, dependencies, DX/tooling,
  docs, direction.
- **Coverage risk:** GPUI-side rendering behavior and the overlay/waveform
  visual path were not re-audited (recently hardened by plan 006); a future
  gpui rev bump is the main unexamined surface.

## Strategic Read

**The critical path is one small formatter fix.** Parakeet TDT 0.6B v2
already won the eval outright — complete >35s transcripts (vs Whisper's
silent 30s truncation), best accuracy on the maintainer's technical
vocabulary, 0.75s short-utterance decode — and is blocked only because its
native punctuation collides with spoken punctuation commands ("Hello comma
world" → `Hello,, world,.`). The handback in plan 004 names the exact fix:
de-duplicate ASR-attached punctuation on words neighboring a spoken command.
That one plan unblocks the default flip, which retires the product's worst
silent failure (30s truncation), which unblocks the live-partials spike.
Nothing else in the repo has this much leverage per unit of work.

**The daemon's failure contract is the biggest correctness debt.** The mic
worker is a single-shot closure: one transient error — mic grabbed by another
app, USB unplug, dropped connection during first model download — exits the
worker permanently and lands in `Unavailable`, a state with no exit
transition. Meanwhile `deliver()` has an unreachable `Err` that the daemon
nevertheless treats as fatal. For a resident background daemon this is the
gap between "hiccup" and "restart the daemon" — worth one hardening batch
before adding more daemon features (insert delivery, partials) on top.

**Verification infrastructure just leveled up; point it at the actual risk.**
The unpushed eval harness (WER/CER thresholds + per-fixture snapshots over
committed, licensed audio) is a genuinely strong baseline — but its corpus is
read prose, which cannot exercise the spoken-punctuation collision that is
the current blocker, and CI never runs it (nor even the harness's pure
scoring-math unit tests, which are feature-gated out of `cargo test`).

**The 2026-07-03 research pass sharpened two things.** First, the
"automatic punctuation without spoken commands" goal is the Parakeet flip —
v2/v3 natively punctuate and capitalize (NVIDIA model cards, verified) —
and for uncased models a 7.1 MB punctuation+casing model is one config
struct away inside the sherpa-onnx crate Dictate already links. No surveyed
tool documents reconciling native punctuation with spoken commands, so the
formatter-fix plan is charting its own path; "spoken commands off by
default under punctuating models" is a live design option there. Second,
the overlay-testing pain has a proven shape: Zed deleted its storybook and
now uses a ~200-line portable `Component`-trait + preview-gallery pattern
(present at the pinned rev); a `dictate debug` window built on it, with
scenario cycling and an in-window WAV→transcribe→format bench, de-risks
both plan 005 and the formatter work.

**What should not happen yet:** insert delivery implementation before the
maintainer answers the spike's four open questions; live partials before the
Parakeet default lands; any packaging/systemd work before structured logging
and the failure-contract fix; a bundled local LLM cleanup stage (the whole
field punts this to external LLMs — no exemplar exists); macOS/Windows
anything.

## Recently shipped

- **Daemon failure-contract hardening** — all five plans in `plans/daemon-failure-contract/` are DONE and committed (infallible delivery, worker error classification, download length verification, mic drop accounting, accept-error backoff).
- **Formatter × native ASR punctuation compatibility** — all three plans in `plans/formatter-punctuation-compat/` are DONE, and the Parakeet default flip landed in 4e00420 ("Flip default model to Parakeet TDT 0.6B v2 and raise the recording cap").
- **README/PLAN/config docs re-true** — landed in 8bcd582.
- **Main publishing cleanup** — the stale "ship the unpushed work" row is being resolved by the current `main` push.
- **Parakeet default flip re-run** — the stale plan 004 re-run row is retired by 4e00420.

## Now

| Opportunity | Audit category | Why now | Impact / leverage | Standards area | Evidence | First strategic slice | Risk / uncertainty | Autonomy boundary | Confidence | Next artifact |
|---|---|---|---|---|---|---|---|---|---|---|
| Long-form fixture clips *(user seed)* | tests | The punctuation plan bank deferred these follow-ups until after the Parakeet flip; under the old Whisper default, long clips silently truncated at 30s | Once Parakeet is default, a >35s fixture makes the corpus test a permanent regression gate against ever reintroducing a 30s-window default | verification | `plans/formatter-punctuation-compat/README.md` follow-ups; measured 2026-07-03 from `tests/fixtures/`; `tests/fixtures/manifest.toml` already records per-fixture provenance + transforms | Add ~35s and ~90–120s fixtures now that the Parakeet default avoids the old `just test-integration` ordering trap; cheapest source: concatenate committed CMU ARCTIC clips (same speaker, provenance already in the manifest, transform recorded as the concat command), with an optional natural 20–35s LibriSpeech test-clean clip (CC BY 4.0, new corpus dir + LICENSE) | Fixture provenance/threshold choices; optional LibriSpeech source adds licensing/provenance work | Routine execution | High | `roadmap-to-improve-plans` |
| Model duration capability + VAD chunking *(added 2026-07-05)* | correctness / direction | Stacks directly on the Parakeet flip: the global cap is now 10 min, but config can still select Whisper models that silently truncate at 30s | Two stages: (a) catalog entries declare a single-pass duration limit and the daemon caps/warns accordingly — cheap honesty fix; (b) VAD-segmented chunking (silero VAD → split at speech boundaries → per-segment offline decode → stitch) removes the limit entirely and flips Whisper's capability to "unlimited via chunking" | boundaries (model window limit is a catalog fact, not a formatter/daemon assumption) | `src/dictation.rs:12` cap now 600s; plan 004 eval table (Whisper 30s truncation); sherpa-onnx long-file VAD examples | Capability field can ride with the long-form fixture work; chunking needs its own small-to-medium plan with the >35s fixtures as its regression gate | Segment-stitching rules (pauses mid-sentence, whitespace/punctuation joins, formatter interaction) need design review | Design review of segment-stitching rules; routine after | High (capability field) / Medium (chunking design) | capability field: fold into long-form fixture batch; chunking: `feature-planning-artifacts` |

## Next

| Opportunity | Audit category | Why next | Impact / leverage | Standards area | Evidence | Prerequisite | Autonomy boundary | Confidence | Likely next artifact |
|---|---|---|---|---|---|---|---|---|---|
| Insert delivery target | direction | Spike verdict decided; the thesis's #1 gap | Text lands in the focused app on niri/wlroots; honest fallback outcome elsewhere | boundaries / effects (semantic insertion vs key emission; `InsertionOutcome` visible to caller) | `spike-insertion-findings.md` verdict + `TextInsertionBackend` sketch + 4 open questions | Maintainer answers the spike's open questions (esp. "insert means insert-or-clipboard?", fcitx5/IBus coexistence) | Human approval on the questions; design review of the seam; routine after | High | `user decision` → `feature-planning-artifacts` |
| `dictate debug` harness *(research)* | DX / tests / direction | Overlay testing currently requires daemon + socket toggling; the harness de-risks plan 005 and the formatter work, and its scenario enum forces phase states to exist as renderable data | Deterministic UI testing without the daemon; interactive WAV→transcribe→format eval bench on the fixture corpus | modules (portable component-preview pattern; seams already exist) | Zed `component`/`component_preview` at pinned rev (verified present; storybook deleted per zed#53511); Dictate seams: overlay handle + `SpectrumLevels`, `audio::load_wav_utterance`, public transcription seam, `DictationFormatter` | None hard; plan before or with plan 005. **Design constraint (user seed): dual-use** — every scenario reachable interactively must also be reachable headless (CLI flags, machine-readable output, capture-and-exit) so agents get the same loop | Design review of the component-registry shape; routine after | High | `feature-planning-artifacts` |
| Overlay phase states + overlay ownership fix | direction / correctness | Only remaining planned product-direction item; deps (hardening 003/005/006) all landed | Legible recording/transcribing/error states; fixes the cancel→start race that records with no visual indicator | state (phase enum drives visuals; one owner for show/hide) | plan 005 (TODO); verified race: `src/daemon.rs:114-116` hides from command thread while worker's `Keep` branch (`:215`) never re-shows | Failure-contract hardening (error states must exist to render); pairs with the debug harness (its scenario selector renders these states) | Routine execution after plan revision | High | revise existing plan 005 to absorb the ownership fix |
| Deterministic formatter increments *(research)* | direction / tech debt | Verified field parity gaps that stay regex/string-level — no LLM, fits `src/text.rs` exactly | Punctuation-aware filler removal (eat the comma the ASR attaches: "um, so"→"so"), inline bracketed-artifact stripping ("(cough)" mid-transcript), sentence-tokenized ~50-word paragraph chunking | domain-modeling (formatter pipeline stages) | VoiceInk `Processing/` pipeline + macparakeet deterministic pipeline (research doc, verified from source); Dictate's `transcript_is_noise` covers whole-utterance junk only | Formatter-compatibility fix landed first (same file, same tests) | Routine execution | High | `roadmap-to-improve-plans` (small; may ride with the formatter-fix batch) |
| Verification upgrades batch | tests / DX | The new harness is strong but half-connected | Scoring math actually tested; catalog/extraction regressions caught before a 600MB download; corpus gate runs in CI | verification | `Cargo.toml:40-43` feature-gates even the pure WER/CER unit tests out of `cargo test`; `src/models.rs:186-223,399-474` zero tests; `ci.yml:70-74` never runs the corpus; apt block duplicated, no `--locked` | Harness stack pushed | Routine execution | High | `roadmap-to-improve-plans` |
| Settings reload path | architecture / DX | Post-003 friction: every config edit requires killing the resident daemon, documented nowhere | Config-file-driven app behaves like one; dictionary/mode/delivery changes apply without restart | effects (when does config take effect?) | `src/daemon.rs:47-51` loads once; `Daemon` holds settings immutably; recognizer built once | Decide shape: reload at utterance boundary vs `dictate reload` socket command (model change stays restart-required) | Design review (reload semantics mid-dictation) | Medium | `feature-planning-artifacts` (small) |

## Later

- **Live partials spike (plan 006)** — already planned; pulls forward the
  moment the Parakeet default lands. Examples-only, parallel-safe.
- **Structured logging (`log`/`tracing`)** — ~27 `eprintln!` sites; becomes
  Now the moment packaging/systemd work starts. AGENTS.md already names the
  crates.
- **Socket ack protocol** — `dictate record start` exits 0 even when the
  command was ignored/busy/unavailable (`src/daemon.rs:39-45`, fire-and-forget;
  daemon never writes to the stream). Product call; pulls forward if hotkey
  scripting or overlay desync complaints appear — and it is the third leg of
  the agentic feedback loop (agents assert daemon state from exit codes
  instead of scraping stderr). Clean-break protocol change is fine (private
  binary pair).
- **Daemon audio injection** *(user seed: agentic feedback loop)* — a
  `record start --from-file x.wav` (or equivalent) path that feeds a fixture
  through the real daemon pipeline (socket → phases → transcription →
  delivery) without audio hardware. Makes the full product flow
  agent-testable end-to-end; design alongside the socket ack.
- **History/database** — PLAN.md behavior-keep; design storage around
  `RawTranscript`/`ProcessedDictation`. After settings reload.
- **App-aware profiles** — needs settings as substrate + per-compositor
  focused-window IPC; design sketch deferred in product-direction README.
- **`OnlinePunctuation` fallback for uncased models** *(research)* — the
  sherpa-onnx crate already wraps it; a 7.1 MB int8 English model adds
  punctuation + casing in ~15–30 ms. Only worth wiring if the catalog keeps
  non-punctuating models user-selectable after the Parakeet flip; gate it
  by the model punctuation capability flag (below). Unmeasured: whether it
  degrades already-punctuated output — never run it unconditionally.
- **Auto-learned dictionary** *(research)* — Wispr Flow auto-populates the
  personal dictionary from user corrections (proper nouns). Needs
  history/database plus a correction signal; design after history lands.
- **LLM rewrite stage (BYOK vs local-GPU vs skip)** — design plan only;
  pointless until insert delivery exists. *(research)*: the survey found
  no tool that bundles a local cleanup model (VoiceInk is BYOK + Ollama;
  Wispr is cloud), so there is no field-proven local model-size floor —
  precede any commitment with an empirical spike (1–4B instruct on target
  CPU). Vocabulary belongs in the prompt as "spelling authority" with an
  anti-over-replacement hedge (VoiceInk's shape) — that is what fixes
  phonetic misses (`Sherpa Onyx`→`sherpa-onnx`) that deterministic
  replacement cannot.
- **Release/packaging + systemd unit** — old signed-artifact pipeline shape
  preserved in history at `dd6db2c175a3`; requires structured logging and the
  failure-contract fix first.
- **Overlay/spectrum pure-logic tests** — `advance_waveform` gating and FFT
  band energy are testable math embedded in the view; fold into plan 005
  execution rather than a separate effort. *(research)*: also spike
  `gpui::TestAppContext`/`VisualTestContext` (present at the pinned rev)
  for headless view tests during the debug-harness plan — capabilities
  unverified; extraction-to-pure-function is the fallback.
- **macOS/Windows ports** — keep seams platform-clean (`DeliveryTarget`,
  overlay handle, future `TextInsertionBackend`); nothing more yet.

## System Upgrades / Standing Policies

| Upgrade or policy | Repeated decision or bottleneck | Proposed durable artifact | Evidence | Owner / next artifact |
|---|---|---|---|---|
| Spoken-punctuation fixture clips | The formatter×model collision was found live, late, in Step 4 of an eval — public corpora (read prose) can never catch it | Self-recorded 16kHz command-word clips under `tests/fixtures/` (rules in `tests/fixtures/README.md` already fit), snapshot-guarded through the real formatter | plan 004 handback; `tests/fixtures/README.md` fixture contract | Recently shipped with `plans/formatter-punctuation-compat/` |
| Model-backed corpus in CI | Quality gate runs only when someone remembers `just test-integration` locally | CI job with cached model dir (`DICTATE_MODEL_DIR`, keyed on model id) | `ci.yml` has no integration job; `Justfile:33-34` | Verification upgrades batch (Next) |
| gpui pin bump checklist | Each bump must re-verify rev-specific workaround knowledge or the overlay silently regresses | Note in AGENTS.md: on gpui bump, re-check the inactive-window ~30fps cap cited at `src/overlay.rs:31-33` and the `LayerShellOptions` API before anything else | `Cargo.toml:12-13` pin (rev `50d001f`, deliberate, ~1 month old — no bump needed now) | One-line AGENTS.md edit, fold into any docs pass |
| Agentic feedback loop (user seed) | Verifying dictation behavior today needs a human to dictate live and watch the overlay — agents (and CI) are locked out of exactly the flows that matter most | Standing policy: every feature plan names how an agent verifies the behavior headlessly (fixture WAV through a CLI seam, snapshot, socket assertion, capture-and-exit debug scenario). Concrete substrates, in leverage order: `dictate transcribe <wav>` CLI (shipped with the formatter fix); dual-use debug harness with `--scenario X --capture out.png --exit` (Next); daemon audio injection (`record start --from-file x.wav`) so the full socket→phase→transcribe→deliver pipeline is agent-drivable; socket ack protocol (Later) so agents can assert daemon state instead of scraping stderr | Fixture corpus + model-backed harness already agent-runnable (`just test`, `just test-integration`); the gaps are live-daemon flows and the overlay | Record as a sentence in AGENTS.md; enforce via plan templates |
| Model punctuation capability flag | "Does this model emit native punctuation?" will be asked again by the formatter, partials, and any future model | Catalog entries declare punctuation behavior (`native / restore / none`); formatter consumes it instead of assuming Whisper; also gates the Later `OnlinePunctuation` fallback *(research)*. *(2026-07-05)*: extend the same capability shape to declare a single-pass duration limit (Whisper ~30s vs Parakeet ~24 min) — see the duration/chunking row in Now | plan 004 handback lingering question; research borrow-list item 3 gives it a second concrete consumer; post-flip, config-selected Whisper models silently truncate under the 10-min cap | Fold into the Now duration-capability work |

## Architecture / Deepening Candidates

| Candidate | Current friction | Audit probe | Deepening direction | Recommendation strength | Evidence | Recommended next artifact |
|---|---|---|---|---|---|---|
| `TextInsertionBackend` seam | Delivery is a flat enum; insertion adds availability, fallback outcomes, and per-compositor variance that `DeliveryTarget` can't express | Seam leaks: caller can't see whether text was inserted or fell back | Deep module behind the sketched trait: semantic insertion distinct from key emission; `InsertionOutcome` visible at the seam | Strong | `spike-insertion-findings.md` seam sketch; `src/delivery.rs` current shape | `feature-planning-artifacts` (with insert delivery) |
| Formatter punctuation-policy stage | Formatter assumes one ASR punctuation style; model-specific behavior is implicit | Bounced-between-modules: fixing the collision requires knowing model behavior in `text.rs` while the model lives in `models.rs` | Explicit policy input to formatting (dedupe/ignore rules) rather than per-model if-branches | Worth exploring | plan 004 handback; `src/text.rs` `DictationContext` | Decide inside the formatter-fix plan |
| `DictationControl` atomic session transitions | Mic worker does check-then-act phase reads with an undo branch; overlay show/hide split across two threads | Interface nearly as complex as implementation at the call site (`src/daemon.rs:151-166`) | One atomic "confirm-still-recording-else-closed" transition owned by the state machine; overlay visibility driven by phase transitions from one owner | Worth exploring | `src/daemon.rs:151-166`; cancel→start race above | Fold into plan 005 revision |
| `deliver()` honest failure contract | `Result` with no reachable `Err`, propagated as fatal by the daemon | Deletion test: the error path can be deleted with zero behavior change | Make infallible (fallback is the contract) or make errors per-utterance | Strong (small) | `src/delivery.rs:28-43`; `src/daemon.rs:183` | Fold into failure-contract hardening |

## Reconcile Existing Plans

| Artifact | Keep / revise / retire | Reason | Next |
|---|---|---|---|
| `plans/gpui-rewrite-hardening/` | Keep as record | All six plans DONE; rejected-list still authoritative | None |
| `plans/product-direction/` 001–003 | Keep as record | DONE and verified in code | None |
| `plans/product-direction/004` | Keep as record | Re-run landed in 4e00420; eval table stays the model-selection reference | None |
| `plans/product-direction/005` | Revise | Still right, but should absorb the overlay show/hide ownership fix (cancel→start race) | Revise before execution |
| `plans/product-direction/006` | Keep | Unblocked by 004 landing | Execute after default flip |
| `PLAN.md` | Keep as record | Docs re-true landed in 8bcd582; behavior-keep inventory still valuable | None |
| Bookmarks: `prototype-v1..5`, `jt-branch-1`, `gpui-native-rewrite`, `feature/osd-architecture-overhaul`, `beads-sync`, `gitbutler/workspace`, `claude/*`, Tauri-era `dependabot/*`, `remove-limits` | Retire | All superseded by the GPUI rewrite now on `main` | Delete local bookmarks; prune origin branches when pushing |

## Not Now / Rejected

Prior rejections in `plans/gpui-rewrite-hardening/README.md` and
`plans/product-direction/README.md` (second inference runtime, portal
GlobalShortcuts, ydotool-as-default, caret-adjacent overlay, enigo, cloud
ASR, checksum pinning, resampler AA filter, VecDeque→Option, FromStr/serde
unification, atomic tearing, et al.) were honored and are **not** restated —
they remain authoritative. New this run:

| Idea | Audit category | Reason | What would change the verdict |
|---|---|---|---|
| Socket bind check-then-act race (two daemons started simultaneously can orphan the live socket) | correctness | Verified real but requires double-fired daemon starts; today's failure is loud (connect error) | A systemd unit / autostart story makes concurrent starts likely — fix with a lock file then |
| Delete dead `VadModel` (`src/models.rs:488-506`) | tech debt | Already rejected by prior audit as fold-into-next-touch | The models.rs test work (Next batch) touches the file — delete it in passing there |
| gpui rev bump | dependencies | Pin is deliberate, ~1 month old, and each bump costs rev-specific re-verification | A needed upstream fix or 3+ months of staleness |
| `cargo audit` one-off | security | Not installed; dep surface is small and mostly vendored-by-pin | Cheap to run once when adding the CI job |
| Formatter fix via per-model if-branches in `text.rs` | architecture | Would leak model identity across the module boundary | Nothing — prefer the punctuation-policy input (see candidates) |

## Recommended Next Move

1. **`roadmap-to-improve-plans` for the Now batch** — long-form fixture clips
   plus the model-duration capability field, so the Parakeet-default corpus
   permanently covers >30s audio and config-selected Whisper models stop
   silently truncating under the 10-minute cap.
2. **One maintainer decision, before insert delivery planning**: answer the
   insertion spike's four open questions (`spike-insertion-findings.md`
   §Open questions). This is a pure judgment call no agent should make.
3. **`feature-planning-artifacts` for VAD chunking** — after the capability
   field and >35s fixture gate exist, design the segment-stitching rules.
4. **`feature-planning-artifacts` for the `dictate debug` harness** — plan
   it before or with plan 005, with the dual-use (interactive + headless
   agent-drivable) constraint baked into the design from the start.

Routine executors can run the long-form fixture and capability-field slices;
VAD segment stitching and insertion semantics need design review. Nothing in
this roadmap needs a spike except what is already speculative in Later.
